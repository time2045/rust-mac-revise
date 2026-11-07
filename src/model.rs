use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub type AppResult<T> = Result<T, String>;

pub fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn normalize_guid(input: &str) -> AppResult<String> {
    Uuid::parse_str(input.trim())
        .map(|id| id.hyphenated().to_string().to_uppercase())
        .map_err(|_| "网卡 GUID 无效".into())
}

pub fn normalize_mac(input: &str) -> AppResult<String> {
    let input = input.trim();
    let bytes = input.as_bytes();
    let compact = if bytes.len() == 12 && bytes.iter().all(u8::is_ascii_hexdigit) {
        input.to_owned()
    } else if bytes.len() == 17
        && matches!(bytes[2], b'-' | b':')
        && bytes.iter().enumerate().all(|(i, b)| {
            if i % 3 == 2 {
                *b == bytes[2]
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        input.chars().filter(|c| *c != '-' && *c != ':').collect()
    } else {
        return Err("请输入 12 位十六进制 MAC，可使用统一的 - 或 : 分隔".into());
    };
    let first = u8::from_str_radix(&compact[..2], 16).map_err(|e| e.to_string())?;
    if first & 1 != 0 {
        return Err("不能使用组播或广播 MAC，首字节必须为偶数".into());
    }
    if compact.chars().all(|c| c == '0') {
        return Err("不能使用全零 MAC".into());
    }
    Ok(compact
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).to_uppercase())
        .collect::<Vec<_>>()
        .join("-"))
}

pub fn random_mac() -> String {
    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[0] = (bytes[0] & 0xfc) | 0x02;
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Adapter {
    pub guid: String,
    pub name: String,
    pub description: String,
    pub pnp_id: String,
    pub mac: Option<String>,
    pub permanent_mac: Option<String>,
    pub configured_mac: Option<String>,
    pub config_error: Option<String>,
    pub admin_enabled: bool,
    pub status: String,
    pub hardware: bool,
    pub can_configure: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub mac: String,
    pub adapter_guid: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    pub id: String,
    pub time: String,
    pub adapter_guid: String,
    pub adapter_name: String,
    pub action: String,
    pub requested_mac: Option<String>,
    pub previous_mac: Option<String>,
    pub actual_mac: Option<String>,
    pub outcome: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub monitor: bool,
    pub interval_seconds: u64,
    #[serde(default)]
    pub auto_repair: BTreeMap<String, bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            monitor: true,
            interval_seconds: 60,
            auto_repair: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        if !matches!(self.interval_seconds, 30 | 60 | 120) {
            return Err("检测间隔只能为 30、60 或 120 秒".into());
        }
        for guid in self.auto_repair.keys() {
            if normalize_guid(guid)? != *guid {
                return Err("自动修复记录中的网卡 GUID 无效".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Override {
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingOperation {
    pub guid: String,
    pub adapter_name: String,
    pub pnp_id: String,
    pub previous_override: Option<Override>,
    pub was_enabled: bool,
    pub previous_mac: Option<String>,
    pub requested_mac: Option<String>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub profiles: Vec<Profile>,
    pub expectations: BTreeMap<String, String>,
    pub history: Vec<History>,
    pub settings: Settings,
    pub pending: Option<PendingOperation>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: vec![],
            expectations: BTreeMap::new(),
            history: vec![],
            settings: Settings::default(),
            pending: None,
        }
    }
}

impl Store {
    pub fn validate(&self) -> AppResult<()> {
        if self.version != 1 {
            return Err("不支持的数据文件版本".into());
        }
        self.settings.validate()?;
        if self.profiles.len() > 500
            || self.history.len() > 300
            || self.expectations.len() > 500
            || self.settings.auto_repair.len() > 500
        {
            return Err("数据条数超过限制".into());
        }
        for p in &self.profiles {
            validate_profile(&p.name, &p.mac, p.adapter_guid.as_deref())?;
        }
        for (guid, mac) in &self.expectations {
            if normalize_guid(guid)? != *guid || normalize_mac(mac)? != *mac {
                return Err("保存的目标地址格式不规范".into());
            }
        }
        for (guid, enabled) in &self.settings.auto_repair {
            if *enabled && !self.expectations.contains_key(guid) {
                return Err("自动修复绑定了一个不存在的目标地址".into());
            }
        }
        if let Some(p) = &self.pending {
            if normalize_guid(&p.guid)? != p.guid || p.pnp_id.is_empty() {
                return Err("恢复记录中的网卡标识无效".into());
            }
            if p.previous_override
                .as_ref()
                .is_some_and(|v| v.bytes.len() > 65536 || v.bytes.len() % 2 != 0)
            {
                return Err("恢复记录中的注册表备份无效".into());
            }
            if let Some(mac) = &p.requested_mac {
                normalize_mac(mac)?;
            }
        }
        Ok(())
    }

    pub fn add_history(&mut self, entry: History) {
        self.history.insert(0, entry);
        self.history.truncate(300);
    }
}

pub fn validate_profile(
    name: &str,
    mac: &str,
    guid: Option<&str>,
) -> AppResult<(String, String, Option<String>)> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("方案名称须为 1～64 个字符，不能包含控制字符".into());
    }
    Ok((
        name.to_owned(),
        normalize_mac(mac)?,
        guid.map(normalize_guid).transpose()?,
    ))
}

#[derive(Serialize)]
pub struct Snapshot {
    pub adapters: Vec<Adapter>,
    pub store: Store,
    pub admin: bool,
    pub data_dir: String,
    pub store_warning: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct OperationResult {
    pub outcome: String,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct ProfileExport {
    pub version: u32,
    pub profiles: Vec<Profile>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_standard_formats() {
        for value in [
            "02abcdef1234",
            "02-ab-cd-ef-12-34",
            "02:ab:cd:ef:12:34",
            " 02ABCDEF1234 ",
        ] {
            assert_eq!(normalize_mac(value).unwrap(), "02-AB-CD-EF-12-34");
        }
        assert!(normalize_mac("74-13-EA-BD-FF-83").is_ok());
    }

    #[test]
    fn rejects_invalid_addresses() {
        for value in [
            "",
            "000000000000",
            "FFFFFFFFFFFF",
            "01ABCDEF1234",
            "02-ab:cd-ef-12-34",
            "02abcdef123",
            "02abcdef12345",
            "02 AB CD EF 12 34",
            "<script>x</script>",
            "中文中文中文",
            "02ABCDEF12ZZ",
            "02abcdef1234\n00",
        ] {
            assert!(normalize_mac(value).is_err(), "{value}");
        }
    }

    #[test]
    fn random_is_local_unicast() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let mac = random_mac();
            assert_eq!(u8::from_str_radix(&mac[..2], 16).unwrap() & 3, 2);
            assert_eq!(normalize_mac(&mac).unwrap(), mac);
            assert!(seen.insert(mac));
        }
    }

    #[test]
    fn guid_is_canonical_and_not_a_shell_fragment() {
        assert_eq!(
            normalize_guid("{9b9945e2-2ec0-41bf-be25-b5c67c827b08}").unwrap(),
            "9B9945E2-2EC0-41BF-BE25-B5C67C827B08"
        );
        assert!(normalize_guid("'; Restart-NetAdapter *; '").is_err());
    }

    #[test]
    fn profile_and_settings_limits() {
        assert!(validate_profile("  ", "02ABCDEF1234", None).is_err());
        assert!(validate_profile(&"中".repeat(65), "02ABCDEF1234", None).is_err());
        assert!(validate_profile("hello\nworld", "02ABCDEF1234", None).is_err());
        assert!(Settings {
            monitor: true,
            interval_seconds: 1,
            ..Settings::default()
        }
        .validate()
        .is_err());
        assert!(Store::default().validate().is_ok());
    }
}
