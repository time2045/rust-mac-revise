use crate::model::{normalize_guid, normalize_mac, Adapter, AppResult, Override};
use serde::Deserialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::{
    Shell::{IsUserAnAdmin, ShellExecuteW},
    WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, SW_SHOWNORMAL},
};
use winreg::{
    enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_SZ},
    RegKey, RegValue,
};

const NETWORK_CLASS: &str = "{4d36e972-e325-11ce-bfc1-08002be10318}";

#[derive(Debug)]
pub struct RestartFailure {
    pub message: String,
    pub timed_out: bool,
}

pub struct MutationLock(pub windows_sys::Win32::Foundation::HANDLE);

impl Drop for MutationLock {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}

pub trait Backend: Send + Sync {
    fn lock_mutation(&self) -> AppResult<MutationLock>;
    fn list(&self) -> AppResult<Vec<Adapter>>;
    fn is_admin(&self) -> bool;
    fn read_override(&self, adapter: &Adapter) -> AppResult<Option<Override>>;
    fn write_override(&self, adapter: &Adapter, value: Option<&Override>) -> AppResult<()>;
    fn restart(&self, guid: &str) -> Result<(), RestartFailure>;
    fn disable(&self, guid: &str) -> AppResult<()>;
    fn settle(&self) {
        thread::sleep(Duration::from_secs(2));
    }
}

pub struct WindowsBackend;

#[derive(Deserialize)]
struct RawAdapter {
    guid: String,
    name: String,
    description: String,
    pnp_id: String,
    mac: String,
    permanent_mac: String,
    admin_enabled: bool,
    status: String,
    hardware: bool,
}

impl Backend for WindowsBackend {
    fn lock_mutation(&self) -> AppResult<MutationLock> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0},
            System::Threading::{CreateMutexW, WaitForSingleObject},
        };
        let name = wide(std::ffi::OsStr::new(
            "Global\\MacAddressManager-NetworkMutation",
        ));
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "无法锁定系统网卡操作：{}",
                std::io::Error::last_os_error()
            ));
        }
        let status = unsafe { WaitForSingleObject(handle, 0) };
        if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
            unsafe {
                CloseHandle(handle);
            }
            return Err("其它程序实例正在修改网卡，请稍后再试".into());
        }
        Ok(MutationLock(handle))
    }

    fn list(&self) -> AppResult<Vec<Adapter>> {
        let output = powershell(include_str!("adapters.ps1"), Duration::from_secs(40))
            .map_err(|e| e.message)?;
        let rows: Vec<RawAdapter> =
            serde_json::from_str(output.trim_start_matches('\u{feff}').trim())
                .map_err(|e| format!("无法解析系统网卡信息：{e}"))?;
        let mut adapters = Vec::new();
        for row in rows {
            let mut adapter = Adapter {
                guid: normalize_guid(&row.guid)?,
                name: row.name,
                description: row.description,
                pnp_id: row.pnp_id,
                mac: normalize_mac(&row.mac).ok(),
                permanent_mac: normalize_mac(&row.permanent_mac).ok(),
                configured_mac: None,
                config_error: None,
                admin_enabled: row.admin_enabled,
                status: row.status,
                hardware: row.hardware,
                can_configure: false,
            };
            if adapter.mac.is_some() || adapter.permanent_mac.is_some() {
                match self.read_override(&adapter) {
                    Ok(value) => {
                        adapter.can_configure =
                            !matches!(adapter.status.as_str(), "Not Present" | "Unknown");
                        if let Some(value) = value {
                            match override_text(&value) {
                                Ok(text) => {
                                    adapter.configured_mac =
                                        Some(normalize_mac(&text).unwrap_or(text))
                                }
                                Err(error) => {
                                    adapter.config_error = Some(error);
                                    adapter.can_configure = false;
                                }
                            }
                        }
                    }
                    Err(error) => adapter.config_error = Some(error),
                }
            }
            adapters.push(adapter);
        }
        adapters.sort_by(|a, b| b.hardware.cmp(&a.hardware).then(a.name.cmp(&b.name)));
        Ok(adapters)
    }

    fn is_admin(&self) -> bool {
        is_admin()
    }

    fn read_override(&self, adapter: &Adapter) -> AppResult<Option<Override>> {
        let key = adapter_key(adapter, KEY_READ)?;
        match key.get_raw_value("NetworkAddress") {
            Ok(value) if value.vtype == REG_SZ => Ok(Some(Override { bytes: value.bytes })),
            Ok(_) => Err("NetworkAddress 不是字符串类型，为避免破坏已有配置已禁止修改".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("无法读取网卡覆盖地址：{error}")),
        }
    }

    fn write_override(&self, adapter: &Adapter, value: Option<&Override>) -> AppResult<()> {
        let key = adapter_key(adapter, KEY_READ | KEY_SET_VALUE)?;
        match value {
            Some(value) => key
                .set_raw_value(
                    "NetworkAddress",
                    &RegValue {
                        bytes: value.bytes.clone(),
                        vtype: REG_SZ,
                    },
                )
                .map_err(|e| format!("无法写入网卡配置：{e}")),
            None => match key.delete_value("NetworkAddress") {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("无法移除网卡覆盖地址：{error}")),
            },
        }?;
        let result = unsafe { windows_sys::Win32::System::Registry::RegFlushKey(key.raw_handle()) };
        if result != 0 {
            return Err(format!(
                "无法将网卡配置写入磁盘：{}",
                std::io::Error::from_raw_os_error(result as i32)
            ));
        }
        Ok(())
    }

    fn restart(&self, guid: &str) -> Result<(), RestartFailure> {
        let guid = normalize_guid(guid).map_err(|message| RestartFailure {
            message,
            timed_out: false,
        })?;
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$g = [guid]'{guid}'
$a = @(Get-NetAdapter -IncludeHidden -ErrorAction Stop | Where-Object {{ [guid]$_.InterfaceGuid -eq $g }})
if ($a.Count -ne 1) {{ throw 'Adapter identity mismatch' }}
if ([int]$a[0].InterfaceAdminStatus -eq 1) {{
    $a[0] | Restart-NetAdapter -Confirm:$false -ErrorAction Stop
}} else {{
    $a[0] | Enable-NetAdapter -Confirm:$false -ErrorAction Stop
}}
"#
        );
        powershell(&script, Duration::from_secs(75)).map(|_| ())
    }

    fn disable(&self, guid: &str) -> AppResult<()> {
        let guid = normalize_guid(guid)?;
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$g = [guid]'{guid}'
$a = @(Get-NetAdapter -IncludeHidden -ErrorAction Stop | Where-Object {{ [guid]$_.InterfaceGuid -eq $g }})
if ($a.Count -ne 1) {{ throw 'Adapter identity mismatch' }}
$a[0] | Disable-NetAdapter -Confirm:$false -ErrorAction Stop
"#
        );
        powershell(&script, Duration::from_secs(75))
            .map(|_| ())
            .map_err(|e| e.message)
    }
}

fn adapter_key(adapter: &Adapter, access: u32) -> AppResult<RegKey> {
    if adapter.pnp_id.is_empty() || adapter.pnp_id.contains('\0') {
        return Err("网卡没有可用的 PnP 标识".into());
    }
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let device = hklm
        .open_subkey_with_flags(
            format!("SYSTEM\\CurrentControlSet\\Enum\\{}", adapter.pnp_id),
            KEY_READ,
        )
        .map_err(|e| format!("无法定位网卡设备：{e}"))?;
    let driver: String = device
        .get_value("Driver")
        .map_err(|e| format!("无法定位网卡驱动：{e}"))?;
    let parts: Vec<_> = driver.split('\\').collect();
    if parts.len() != 2
        || !parts[0].eq_ignore_ascii_case(NETWORK_CLASS)
        || parts[1].len() != 4
        || !parts[1].bytes().all(|b| b.is_ascii_digit())
    {
        return Err("设备不属于可配置的网络适配器类".into());
    }
    let key = hklm
        .open_subkey_with_flags(
            format!("SYSTEM\\CurrentControlSet\\Control\\Class\\{driver}"),
            access,
        )
        .map_err(|e| format!("无法打开网卡驱动配置：{e}"))?;
    let registry_guid: String = key
        .get_value("NetCfgInstanceId")
        .map_err(|e| format!("驱动没有网卡标识：{e}"))?;
    if normalize_guid(&registry_guid)? != adapter.guid {
        return Err("设备和驱动的网卡 GUID 不一致，已停止操作".into());
    }
    Ok(key)
}

pub fn override_from_mac(mac: &str) -> Override {
    let compact = mac.replace('-', "");
    Override {
        bytes: compact
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect(),
    }
}

fn override_text(value: &Override) -> AppResult<String> {
    if value.bytes.len() % 2 != 0 {
        return Err("网卡覆盖地址的 UTF-16 数据无效".into());
    }
    let words: Vec<_> = value
        .bytes
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    String::from_utf16(&words)
        .map(|s| s.trim_end_matches('\0').to_owned())
        .map_err(|_| "网卡覆盖地址的编码无效".into())
}

fn powershell(script: &str, timeout: Duration) -> Result<String, RestartFailure> {
    let fail = |message| RestartFailure {
        message,
        timed_out: false,
    };
    let system =
        std::env::var_os("SystemRoot").ok_or_else(|| fail("缺少 SystemRoot 环境变量".into()))?;
    let program = PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let modules = program
        .parent()
        .ok_or_else(|| fail("PowerShell 路径无效".into()))?
        .join("Modules");
    let script = format!("$ErrorActionPreference = 'Stop'\n$env:PSModulePath = Join-Path $PSHOME 'Modules'\nImport-Module (Join-Path $PSHOME 'Modules\\NetAdapter\\NetAdapter.psd1') -ErrorAction Stop\n{script}");
    let mut child = Command::new(program)
        .env("PSModulePath", modules)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| fail(format!("无法启动 Windows PowerShell：{e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("无法读取系统命令输出".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| fail("无法读取系统命令错误".into()))?;
    let out_thread = thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let err_thread = thread::spawn(move || {
        let mut out = Vec::new();
        stderr.read_to_end(&mut out).map(|_| out)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RestartFailure {
                    message: format!("无法确认系统命令状态：{error}"),
                    timed_out: true,
                });
            }
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RestartFailure {
                message: "系统命令超时，设备操作结果尚不确定；请检查网卡状态后使用恢复上次配置"
                    .into(),
                timed_out: true,
            });
        }
        thread::sleep(Duration::from_millis(100));
    };
    let output = out_thread
        .join()
        .map_err(|_| fail("读取系统输出失败".into()))?
        .map_err(|e| fail(e.to_string()))?;
    let error = err_thread
        .join()
        .map_err(|_| fail("读取系统错误失败".into()))?
        .map_err(|e| fail(e.to_string()))?;
    if !status.success() {
        return Err(fail(format!(
            "系统命令失败：{}",
            String::from_utf8_lossy(&error).trim()
        )));
    }
    String::from_utf8(output).map_err(|e| fail(format!("系统输出不是 UTF-8：{e}")))
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn shell_open(verb: &str, target: &Path, arguments: Option<&str>) -> AppResult<()> {
    let verb = wide(std::ffi::OsStr::new(verb));
    let file = wide(target.as_os_str());
    let arguments = arguments.map(|s| wide(std::ffi::OsStr::new(s)));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            arguments.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err(format!(
            "Windows 未完成操作（代码 {result}），可能已取消授权"
        ));
    }
    Ok(())
}

pub fn relaunch_admin() -> AppResult<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    shell_open("runas", &exe, Some("--elevated"))
}

pub fn open_wifi_settings() -> AppResult<()> {
    shell_open("open", Path::new("ms-settings:network-wifi"), None)
}

pub fn open_url(url: &str) -> AppResult<()> {
    if !url.starts_with("https://github.com/time2045/rust-mac-revise") {
        return Err("不允许的链接".into());
    }
    shell_open("open", Path::new(url), None)
}

pub fn open_path(path: &Path) -> AppResult<()> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    shell_open("open", path, None)
}

pub fn show_error(message: &str) {
    let message = wide(std::ffi::OsStr::new(message));
    let title = wide(std::ffi::OsStr::new("MAC 地址管理器"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn lock_instance(dir: &Path) -> AppResult<File> {
    fs::create_dir_all(dir).map_err(|e| format!("无法创建数据目录：{e}"))?;
    let wait = std::env::args().any(|arg| arg == "--elevated");
    let start = Instant::now();
    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(dir.join("instance.lock"))
        {
            Ok(file) => return Ok(file),
            Err(_) if wait && start.elapsed() < Duration::from_secs(8) => {
                thread::sleep(Duration::from_millis(200))
            }
            Err(error) => {
                return Err(format!(
                    "程序可能已经运行，或数据目录不可写。请先关闭已打开的窗口。{error}"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_encoding_roundtrip() {
        let raw = override_from_mac("02-AB-CD-EF-12-34");
        assert_eq!(override_text(&raw).unwrap(), "02ABCDEF1234");
        assert_eq!(raw.bytes.len(), 26);
        assert!(override_text(&Override { bytes: vec![1] }).is_err());
    }
}
