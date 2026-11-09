use crate::{
    model::*,
    platform::{override_from_mac, Backend},
    store::Persistence,
};
use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;

pub struct Service<B: Backend> {
    backend: B,
    store: Mutex<Persistence>,
}

impl<B: Backend> Service<B> {
    pub fn new(backend: B, store: Persistence) -> Self {
        Self {
            backend,
            store: Mutex::new(store),
        }
    }

    fn lock(&self) -> AppResult<MutexGuard<'_, Persistence>> {
        self.store
            .lock()
            .map_err(|_| "数据状态异常，请重启程序".into())
    }

    pub fn data_dir(&self) -> AppResult<PathBuf> {
        Ok(self.lock()?.dir.clone())
    }

    pub fn snapshot(&self) -> AppResult<Snapshot> {
        let store = self.lock()?;
        Ok(Snapshot {
            adapters: self.backend.list()?,
            store: store.data.clone(),
            admin: self.backend.is_admin(),
            autostart: self.backend.is_autostart_enabled(),
            data_dir: store.dir.to_string_lossy().into(),
            store_warning: store.warning.clone(),
        })
    }

    pub fn save_profile(&self, name: &str, mac: &str, guid: Option<&str>) -> AppResult<Store> {
        let (name, mac, guid) = validate_profile(name, mac, guid)?;
        let mut store = self.lock()?;
        let mut next = store.data.clone();
        upsert_profile(&mut next.profiles, name, mac, guid)?;
        store.commit(next)?;
        Ok(store.data.clone())
    }

    pub fn delete_profile(&self, id: &str) -> AppResult<Store> {
        let mut store = self.lock()?;
        let mut next = store.data.clone();
        let index = next
            .profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or("地址方案不存在")?;
        next.profiles.remove(index);
        store.commit(next)?;
        Ok(store.data.clone())
    }

    pub fn import_profiles(&self, content: &str) -> AppResult<Store> {
        if content.len() > 1024 * 1024 {
            return Err("导入文件不能超过 1 MB".into());
        }
        let imported: ProfileExport = serde_json::from_str(content.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("地址方案文件格式错误：{e}"))?;
        if imported.version != 1 || imported.profiles.len() > 500 {
            return Err("不支持的方案版本或方案超过 500 条".into());
        }
        let mut store = self.lock()?;
        let mut next = store.data.clone();
        for p in imported.profiles {
            let (name, mac, guid) = validate_profile(&p.name, &p.mac, p.adapter_guid.as_deref())?;
            upsert_profile(&mut next.profiles, name, mac, guid)?;
        }
        store.commit(next)?;
        Ok(store.data.clone())
    }

    pub fn export_profiles(&self) -> AppResult<String> {
        let store = self.lock()?;
        store.ensure_writable()?;
        serde_json::to_string_pretty(&ProfileExport {
            version: 1,
            profiles: store.data.profiles.clone(),
        })
        .map_err(|e| e.to_string())
    }

    pub fn save_settings(&self, settings: Settings) -> AppResult<Store> {
        settings.validate()?;
        let mut store = self.lock()?;
        let mut next = store.data.clone();
        next.settings.monitor = settings.monitor;
        next.settings.interval_seconds = settings.interval_seconds;
        store.commit(next)?;
        Ok(store.data.clone())
    }

    pub fn set_autostart(&self, enabled: bool) -> AppResult<bool> {
        self.backend.set_autostart(enabled)
    }

    pub fn clear_history(&self) -> AppResult<Store> {
        let mut store = self.lock()?;
        store.ensure_writable()?;
        let mut next = store.data.clone();
        next.history.clear();
        store.commit(next)?;
        Ok(store.data.clone())
    }

    pub fn set_auto_repair(&self, guid: &str, enabled: bool) -> AppResult<Store> {
        let guid = normalize_guid(guid)?;
        let mut store = self.lock()?;
        store.ensure_writable()?;
        if enabled && !store.data.expectations.contains_key(&guid) {
            return Err("请先应用地址，再启用自动修复".into());
        }
        let mut next = store.data.clone();
        if enabled {
            next.settings.auto_repair.insert(guid, true);
        } else {
            next.settings.auto_repair.remove(&guid);
        }
        store.commit(next)?;
        Ok(store.data.clone())
    }

    pub fn apply(&self, guid: &str, mac: Option<&str>) -> AppResult<OperationResult> {
        self.apply_inner(guid, mac, None)
    }

    pub fn auto_repair(&self, guid: &str, mac: &str) -> AppResult<OperationResult> {
        self.apply_inner(guid, Some(mac), Some("auto_repair"))
    }

    fn apply_inner(
        &self,
        guid: &str,
        mac: Option<&str>,
        action: Option<&str>,
    ) -> AppResult<OperationResult> {
        let guid = normalize_guid(guid)?;
        let target = mac.map(normalize_mac).transpose()?;
        let mut store = self.lock()?;
        store.ensure_writable()?;
        if !self.backend.is_admin() {
            return Err("修改网卡需要管理员权限".into());
        }
        if store.data.pending.is_some() {
            return Err("存在未完成的操作，请先恢复上次配置".into());
        }
        let _machine_lock = self.backend.lock_mutation()?;
        let adapters = self.backend.list()?;
        let adapter = adapters
            .iter()
            .find(|a| a.guid == guid)
            .ok_or("所选网卡已不存在，请刷新列表")?;
        if !adapter.can_configure {
            return Err("该网卡当前不可配置，请检查驱动或设备状态".into());
        }
        if target.as_ref().is_some_and(|target| {
            adapters
                .iter()
                .any(|a| a.guid != guid && a.mac.as_ref() == Some(target))
        }) {
            return Err("另一块网卡正在使用这个 MAC，请选择不同的地址".into());
        }
        let pending = PendingOperation {
            guid: guid.clone(),
            adapter_name: adapter.name.clone(),
            pnp_id: adapter.pnp_id.clone(),
            previous_override: self.backend.read_override(adapter)?,
            was_enabled: adapter.admin_enabled,
            previous_mac: adapter.mac.clone(),
            requested_mac: target.clone(),
            action: action.map(str::to_owned).unwrap_or_else(|| {
                if target.is_some() {
                    "apply".into()
                } else {
                    "restore".into()
                }
            }),
        };
        if pending.requested_mac.is_none() && pending.previous_override.is_none() {
            return self.finish(
                &mut store,
                &pending,
                "success",
                "当前没有手动覆盖地址，无需恢复；未重启网卡。系统随机硬件地址设置保持不变。",
                adapter.mac.clone(),
                true,
            );
        }
        let mut next = store.data.clone();
        next.pending = Some(pending.clone());
        store.commit(next)?;
        let value = target.as_deref().map(override_from_mac);
        let write = self
            .backend
            .write_override(adapter, value.as_ref())
            .and_then(|_| {
                if self.backend.read_override(adapter)? != value {
                    return Err("注册表回读与写入值不一致".into());
                }
                Ok(())
            });
        if let Err(error) = write {
            return self.handle_failure(&mut store, &pending, &error);
        }
        if !pending.was_enabled {
            return self.finish(
                &mut store,
                &pending,
                "pending",
                "配置已保存；网卡保持禁用，需下次手动启用后确认实际地址。",
                adapter.mac.clone(),
                true,
            );
        }
        if let Err(error) = self.backend.restart(&guid) {
            if error.timed_out {
                return self.finish(
                    &mut store,
                    &pending,
                    "recovery_required",
                    &error.message,
                    None,
                    false,
                );
            }
            return self.handle_failure(&mut store, &pending, &error.message);
        }
        let verification = self.verify(&pending, target.as_deref(), false);
        match verification {
            Ok(actual) => {
                let (outcome, message) = if let Some(mac) = &target {
                    ("success", format!("已应用并确认当前 MAC 为 {mac}。"))
                } else if actual.permanent_mac.is_none() || actual.mac != actual.permanent_mac {
                    ("pending", "手动覆盖已移除且网卡已启用；当前地址未确认与永久地址一致，请检查系统随机硬件地址设置或驱动。".into())
                } else {
                    (
                        "success",
                        "已移除手动覆盖，并确认当前地址与驱动报告的永久地址一致。".into(),
                    )
                };
                self.finish(&mut store, &pending, outcome, &message, actual.mac, true)
            }
            Err(error) => self.handle_failure(&mut store, &pending, &error),
        }
    }

    fn find_identity(&self, pending: &PendingOperation) -> AppResult<Adapter> {
        let adapter = self
            .backend
            .list()?
            .into_iter()
            .find(|a| a.guid == pending.guid)
            .ok_or("网卡已移除，无法完成校验或恢复")?;
        if !adapter.pnp_id.eq_ignore_ascii_case(&pending.pnp_id) {
            return Err("网卡设备标识发生变化，已停止操作".into());
        }
        Ok(adapter)
    }

    fn verify(
        &self,
        pending: &PendingOperation,
        target: Option<&str>,
        restoring_backup: bool,
    ) -> AppResult<Adapter> {
        let mut reason = String::from("驱动没有报告预期 MAC");
        for attempt in 0..5 {
            if attempt > 0 {
                self.backend.settle();
            }
            let result = self.find_identity(pending).and_then(|adapter| {
                if adapter.admin_enabled != pending.was_enabled {
                    return Err("网卡启用状态尚未恢复".into());
                }
                let expected_override = if restoring_backup {
                    pending.previous_override.clone()
                } else { pending.requested_mac.as_deref().map(override_from_mac) };
                if self.backend.read_override(&adapter)? != expected_override {
                    return Err("网卡覆盖配置回读不一致".into());
                }
                if pending.was_enabled && target.is_some_and(|mac| adapter.mac.as_deref() != Some(mac)) {
                    return Err(format!("驱动未接受目标地址，当前 MAC：{}。可能受驱动限制或 Windows 随机硬件地址影响", adapter.mac.as_deref().unwrap_or("未知")));
                }
                Ok(adapter)
            });
            match result {
                Ok(adapter) => return Ok(adapter),
                Err(error) => reason = error,
            }
        }
        Err(reason)
    }

    fn rollback(&self, pending: &PendingOperation) -> AppResult<Adapter> {
        let current = self.find_identity(pending)?;
        self.backend
            .write_override(&current, pending.previous_override.as_ref())?;
        if pending.was_enabled {
            self.backend.restart(&pending.guid).map_err(|e| e.message)?;
        } else if current.admin_enabled {
            self.backend.disable(&pending.guid)?;
        }
        self.verify(pending, None, true)
    }

    fn handle_failure(
        &self,
        store: &mut Persistence,
        pending: &PendingOperation,
        error: &str,
    ) -> AppResult<OperationResult> {
        match self.rollback(pending) {
            Ok(adapter) => self.finish(store, pending, "rolled_back", &format!("未能应用：{error}。{}", recovery_message(pending, &adapter)), adapter.mac, false),
            Err(rollback) => self.finish(store, pending, "recovery_required", &format!("操作失败：{error}。恢复尚未确认：{rollback}。原配置备份已保留，请检查设备后使用恢复上次配置。"), None, false),
        }
    }

    pub fn recover(&self) -> AppResult<OperationResult> {
        let mut store = self.lock()?;
        store.ensure_writable()?;
        if !self.backend.is_admin() {
            return Err("恢复配置需要管理员权限".into());
        }
        let pending = store.data.pending.clone().ok_or("没有需要恢复的操作")?;
        let _machine_lock = self.backend.lock_mutation()?;
        match self.rollback(&pending) {
            Ok(adapter) => self.finish(
                &mut store,
                &pending,
                "rolled_back",
                &recovery_message(&pending, &adapter),
                adapter.mac,
                false,
            ),
            Err(error) => self.finish(
                &mut store,
                &pending,
                "recovery_required",
                &format!("恢复未完成：{error}。备份仍然保留。"),
                None,
                false,
            ),
        }
    }

    fn finish(
        &self,
        store: &mut Persistence,
        pending: &PendingOperation,
        outcome: &str,
        message: &str,
        actual: Option<String>,
        accepted: bool,
    ) -> AppResult<OperationResult> {
        let mut next = store.data.clone();
        if outcome != "recovery_required" {
            next.pending = None;
        }
        if accepted {
            match &pending.requested_mac {
                Some(mac) => {
                    next.expectations.insert(pending.guid.clone(), mac.clone());
                }
                None => {
                    next.expectations.remove(&pending.guid);
                    next.settings.auto_repair.remove(&pending.guid);
                }
            }
        }
        next.add_history(History {
            id: Uuid::new_v4().to_string(),
            time: timestamp(),
            adapter_guid: pending.guid.clone(),
            adapter_name: pending.adapter_name.clone(),
            action: pending.action.clone(),
            requested_mac: pending.requested_mac.clone(),
            previous_mac: pending.previous_mac.clone(),
            actual_mac: actual,
            outcome: outcome.into(),
            message: message.into(),
        });
        store.commit(next).map_err(|error| {
            format!("{message} 但无法保存操作结果：{error}。原配置恢复记录仍保留，请勿重复应用。")
        })?;
        Ok(OperationResult {
            outcome: outcome.into(),
            message: message.into(),
        })
    }
}

fn recovery_message(pending: &PendingOperation, adapter: &Adapter) -> String {
    let mut message = String::from("已恢复并校验操作前的覆盖配置和启用状态。");
    if pending.was_enabled && adapter.mac != pending.previous_mac {
        message.push_str(&format!(
            "当前 MAC 为 {}，与操作前不同；系统随机硬件地址或驱动可能影响实际地址。",
            adapter.mac.as_deref().unwrap_or("未知")
        ));
    }
    message
}

fn upsert_profile(
    profiles: &mut Vec<Profile>,
    name: String,
    mac: String,
    guid: Option<String>,
) -> AppResult<()> {
    if let Some(existing) = profiles
        .iter_mut()
        .find(|p| p.name.to_lowercase() == name.to_lowercase() && p.adapter_guid == guid)
    {
        existing.name = name;
        existing.mac = mac;
    } else {
        if profiles.len() >= 500 {
            return Err("最多保存 500 个地址方案".into());
        }
        profiles.push(Profile {
            id: Uuid::new_v4().to_string(),
            name,
            mac,
            adapter_guid: guid,
            created_at: timestamp(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::RestartFailure;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const GUID: &str = "11111111-1111-4111-8111-111111111111";
    const ORIGINAL: &str = "74-13-EA-BD-FF-83";
    const TARGET: &str = "02-11-22-33-44-55";

    struct FakeBackend {
        adapter: Mutex<Adapter>,
        value: Mutex<Option<Override>>,
        admin: bool,
        rejects: bool,
        timeout: bool,
        autostart: AtomicBool,
        restart_count: AtomicUsize,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                adapter: Mutex::new(Adapter {
                    guid: GUID.into(),
                    name: "测试网卡".into(),
                    description: "测试驱动".into(),
                    pnp_id: "TEST\\NIC".into(),
                    mac: Some(ORIGINAL.into()),
                    permanent_mac: Some(ORIGINAL.into()),
                    configured_mac: None,
                    config_error: None,
                    admin_enabled: true,
                    status: "Up".into(),
                    hardware: true,
                    can_configure: true,
                }),
                value: Mutex::new(None),
                admin: true,
                rejects: false,
                timeout: false,
                autostart: AtomicBool::new(false),
                restart_count: AtomicUsize::new(0),
            }
        }
    }

    impl Backend for FakeBackend {
        fn lock_mutation(&self) -> AppResult<crate::platform::MutationLock> {
            Ok(crate::platform::MutationLock(std::ptr::null_mut()))
        }
        fn list(&self) -> AppResult<Vec<Adapter>> {
            Ok(vec![self.adapter.lock().unwrap().clone()])
        }
        fn is_admin(&self) -> bool {
            self.admin
        }
        fn is_autostart_enabled(&self) -> bool {
            self.autostart.load(Ordering::SeqCst)
        }
        fn set_autostart(&self, enabled: bool) -> AppResult<bool> {
            self.autostart.store(enabled, Ordering::SeqCst);
            Ok(enabled)
        }
        fn read_override(&self, _: &Adapter) -> AppResult<Option<Override>> {
            Ok(self.value.lock().unwrap().clone())
        }
        fn write_override(&self, _: &Adapter, value: Option<&Override>) -> AppResult<()> {
            *self.value.lock().unwrap() = value.cloned();
            Ok(())
        }
        fn restart(&self, _: &str) -> Result<(), RestartFailure> {
            self.restart_count.fetch_add(1, Ordering::SeqCst);
            if self.timeout {
                return Err(RestartFailure {
                    message: "timeout".into(),
                    timed_out: true,
                });
            }
            let value = self.value.lock().unwrap();
            if !self.rejects {
                let mac = value
                    .as_ref()
                    .map(|v| {
                        let words: Vec<_> = v
                            .bytes
                            .chunks_exact(2)
                            .map(|p| u16::from_le_bytes([p[0], p[1]]))
                            .collect();
                        normalize_mac(String::from_utf16(&words).unwrap().trim_end_matches('\0'))
                            .unwrap()
                    })
                    .unwrap_or_else(|| ORIGINAL.into());
                let mut adapter = self.adapter.lock().unwrap();
                adapter.mac = Some(mac);
                adapter.admin_enabled = true;
            }
            Ok(())
        }
        fn disable(&self, _: &str) -> AppResult<()> {
            self.adapter.lock().unwrap().admin_enabled = false;
            Ok(())
        }
        fn settle(&self) {}
    }

    fn service(backend: FakeBackend) -> Service<FakeBackend> {
        Service::new(
            backend,
            Persistence::load(
                std::env::temp_dir().join(format!("mac-manager-test-{}", Uuid::new_v4())),
            ),
        )
    }

    fn cleanup(s: &Service<FakeBackend>) {
        let _ = std::fs::remove_dir_all(s.data_dir().unwrap());
    }

    #[test]
    fn successful_apply_is_verified_and_persisted() {
        let s = service(FakeBackend::new());
        assert_eq!(s.apply(GUID, Some(TARGET)).unwrap().outcome, "success");
        let state = s.snapshot().unwrap();
        assert_eq!(state.store.expectations[GUID], TARGET);
        assert!(state.store.pending.is_none());
        assert_eq!(state.store.history.len(), 1);
        assert_eq!(state.adapters[0].mac.as_deref(), Some(TARGET));
        cleanup(&s);
    }

    #[test]
    fn rejected_driver_rolls_back() {
        let mut backend = FakeBackend::new();
        backend.rejects = true;
        let s = service(backend);
        assert_eq!(s.apply(GUID, Some(TARGET)).unwrap().outcome, "rolled_back");
        assert!(s.backend.value.lock().unwrap().is_none());
        assert!(s.snapshot().unwrap().store.expectations.is_empty());
        assert!(s.snapshot().unwrap().store.pending.is_none());
        cleanup(&s);
    }

    #[test]
    fn disabled_adapter_is_never_enabled() {
        let backend = FakeBackend::new();
        backend.adapter.lock().unwrap().admin_enabled = false;
        let s = service(backend);
        assert_eq!(s.apply(GUID, Some(TARGET)).unwrap().outcome, "pending");
        assert_eq!(s.backend.restart_count.load(Ordering::SeqCst), 0);
        assert!(!s.snapshot().unwrap().adapters[0].admin_enabled);
        assert_eq!(s.snapshot().unwrap().store.expectations[GUID], TARGET);
        cleanup(&s);
    }

    #[test]
    fn restore_deletes_override_not_writes_permanent() {
        let s = service(FakeBackend::new());
        s.apply(GUID, Some(TARGET)).unwrap();
        assert_eq!(s.apply(GUID, None).unwrap().outcome, "success");
        assert!(s.backend.value.lock().unwrap().is_none());
        assert!(s.snapshot().unwrap().store.expectations.is_empty());
        cleanup(&s);
    }

    #[test]
    fn restoring_without_override_does_not_restart() {
        let s = service(FakeBackend::new());
        let result = s.apply(GUID, None).unwrap();
        assert_eq!(result.outcome, "success");
        assert!(result.message.contains("未重启网卡"));
        assert_eq!(s.backend.restart_count.load(Ordering::SeqCst), 0);
        cleanup(&s);
    }

    #[test]
    fn timeouts_keep_journal_and_block_more_writes() {
        let mut backend = FakeBackend::new();
        backend.timeout = true;
        let s = service(backend);
        assert_eq!(
            s.apply(GUID, Some(TARGET)).unwrap().outcome,
            "recovery_required"
        );
        assert!(s.snapshot().unwrap().store.pending.is_some());
        assert!(s.apply(GUID, Some(TARGET)).is_err());
        assert_eq!(s.backend.restart_count.load(Ordering::SeqCst), 1);
        assert!(Persistence::load(s.data_dir().unwrap())
            .data
            .pending
            .is_some());
        cleanup(&s);
    }

    #[test]
    fn non_admin_cannot_write_but_can_save_profiles() {
        let mut backend = FakeBackend::new();
        backend.admin = false;
        let s = service(backend);
        assert!(s.apply(GUID, Some(TARGET)).is_err());
        assert!(s.backend.value.lock().unwrap().is_none());
        assert_eq!(
            s.save_profile("办公", TARGET, Some(GUID))
                .unwrap()
                .profiles
                .len(),
            1
        );
        cleanup(&s);
    }

    #[test]
    fn profiles_upsert_and_import_atomically() {
        let s = service(FakeBackend::new());
        s.save_profile("办公", TARGET, Some(GUID)).unwrap();
        s.save_profile("办公", "02-A1-A2-A3-A4-A5", Some(GUID))
            .unwrap();
        let profiles = s.snapshot().unwrap().store.profiles;
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].mac, "02-A1-A2-A3-A4-A5");
        let exported = s.export_profiles().unwrap();
        assert_eq!(s.import_profiles(&exported).unwrap().profiles.len(), 1);
        let mut bad = profiles[0].clone();
        bad.name = "invalid".into();
        bad.mac = "bad".into();
        let content = serde_json::to_string(&ProfileExport {
            version: 1,
            profiles: vec![bad],
        })
        .unwrap();
        assert!(s.import_profiles(&content).is_err());
        assert_eq!(s.snapshot().unwrap().store.profiles.len(), 1);
        assert!(s.import_profiles("{}").is_err());
        cleanup(&s);
    }

    #[test]
    fn recovering_saved_journal_restores_original() {
        let s = service(FakeBackend::new());
        let mut store = s.lock().unwrap();
        let mut next = store.data.clone();
        next.pending = Some(PendingOperation {
            guid: GUID.into(),
            adapter_name: "测试网卡".into(),
            pnp_id: "TEST\\NIC".into(),
            previous_override: None,
            was_enabled: true,
            previous_mac: Some(ORIGINAL.into()),
            requested_mac: Some(TARGET.into()),
            action: "apply".into(),
        });
        store.commit(next).unwrap();
        drop(store);
        *s.backend.value.lock().unwrap() = Some(override_from_mac(TARGET));
        assert_eq!(s.recover().unwrap().outcome, "rolled_back");
        assert!(s.backend.value.lock().unwrap().is_none());
        assert!(s.snapshot().unwrap().store.pending.is_none());
        cleanup(&s);
    }

    #[test]
    fn corrupt_store_blocks_network_mutation() {
        let dir = std::env::temp_dir().join(format!("mac-manager-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.json"), "broken").unwrap();
        let s = Service::new(FakeBackend::new(), Persistence::load(dir));
        assert!(s.apply(GUID, Some(TARGET)).is_err());
        assert!(s.backend.value.lock().unwrap().is_none());
        assert_eq!(s.backend.restart_count.load(Ordering::SeqCst), 0);
        cleanup(&s);
    }

    #[test]
    fn backup_recovery_preserves_exact_registry_bytes() {
        let backend = FakeBackend::new();
        backend.adapter.lock().unwrap().admin_enabled = false;
        let s = service(backend);
        let raw = Override {
            bytes: "02aabbccddee\0\0"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        };
        let mut store = s.lock().unwrap();
        let mut next = store.data.clone();
        next.pending = Some(PendingOperation {
            guid: GUID.into(),
            adapter_name: "测试网卡".into(),
            pnp_id: "TEST\\NIC".into(),
            previous_override: Some(raw.clone()),
            was_enabled: false,
            previous_mac: Some(ORIGINAL.into()),
            requested_mac: Some(TARGET.into()),
            action: "apply".into(),
        });
        store.commit(next).unwrap();
        drop(store);
        s.backend.adapter.lock().unwrap().admin_enabled = true;
        assert_eq!(s.recover().unwrap().outcome, "rolled_back");
        assert!(!s.backend.adapter.lock().unwrap().admin_enabled);
        assert_eq!(*s.backend.value.lock().unwrap(), Some(raw));
        assert_eq!(s.backend.restart_count.load(Ordering::SeqCst), 0);
        cleanup(&s);
    }

    #[test]
    fn randomized_current_address_does_not_trap_recovery() {
        let mut backend = FakeBackend::new();
        backend.rejects = true;
        backend.adapter.lock().unwrap().mac = Some(TARGET.into());
        let s = service(backend);
        let mut store = s.lock().unwrap();
        let mut next = store.data.clone();
        next.pending = Some(PendingOperation {
            guid: GUID.into(),
            adapter_name: "测试网卡".into(),
            pnp_id: "TEST\\NIC".into(),
            previous_override: None,
            was_enabled: true,
            previous_mac: Some(ORIGINAL.into()),
            requested_mac: Some(TARGET.into()),
            action: "apply".into(),
        });
        store.commit(next).unwrap();
        drop(store);
        let result = s.recover().unwrap();
        assert_eq!(result.outcome, "rolled_back");
        assert!(result.message.contains("与操作前不同"));
        assert!(s.snapshot().unwrap().store.pending.is_none());
        cleanup(&s);
    }

    #[test]
    fn auto_repair_requires_target_and_survives_settings_save() {
        let s = service(FakeBackend::new());
        assert!(s.set_auto_repair(GUID, true).is_err());
        s.apply(GUID, Some(TARGET)).unwrap();
        s.set_auto_repair(GUID, true).unwrap();
        assert!(s.snapshot().unwrap().store.settings.auto_repair[GUID]);
        s.save_settings(Settings {
            monitor: false,
            interval_seconds: 120,
            ..Settings::default()
        })
        .unwrap();
        let store = s.snapshot().unwrap().store;
        assert!(!store.settings.monitor);
        assert_eq!(store.settings.interval_seconds, 120);
        assert!(store.settings.auto_repair[GUID]);
        s.apply(GUID, None).unwrap();
        assert!(!s
            .snapshot()
            .unwrap()
            .store
            .settings
            .auto_repair
            .contains_key(GUID));
        cleanup(&s);
    }

    #[test]
    fn clear_history_keeps_profiles_and_expectations() {
        let s = service(FakeBackend::new());
        s.apply(GUID, Some(TARGET)).unwrap();
        s.save_profile("办公", TARGET, Some(GUID)).unwrap();
        assert_eq!(s.snapshot().unwrap().store.history.len(), 1);
        let store = s.clear_history().unwrap();
        assert!(store.history.is_empty());
        assert_eq!(store.profiles.len(), 1);
        assert_eq!(store.expectations[GUID], TARGET);
        assert!(Persistence::load(s.data_dir().unwrap())
            .data
            .history
            .is_empty());
        cleanup(&s);
    }

    #[test]
    fn autostart_preference_roundtrip() {
        let s = service(FakeBackend::new());
        assert!(!s.snapshot().unwrap().autostart);
        assert!(s.set_autostart(true).unwrap());
        assert!(s.snapshot().unwrap().autostart);
        assert!(!s.set_autostart(false).unwrap());
        assert!(!s.snapshot().unwrap().autostart);
        cleanup(&s);
    }

    #[test]
    fn invalid_input_has_no_side_effects() {
        let s = service(FakeBackend::new());
        assert!(s.apply(GUID, Some("FFFFFFFFFFFF")).is_err());
        assert!(s.apply("not-a-guid", Some(TARGET)).is_err());
        assert!(s.snapshot().unwrap().store.pending.is_none());
        assert!(s.backend.value.lock().unwrap().is_none());
        cleanup(&s);
    }
}
