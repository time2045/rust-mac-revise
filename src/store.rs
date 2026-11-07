use crate::model::{AppResult, Store};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};
use uuid::Uuid;

pub fn data_dir() -> AppResult<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("MacAddressManager"))
        .ok_or_else(|| "未找到当前用户的 LOCALAPPDATA 目录".into())
}

pub struct Persistence {
    pub data: Store,
    pub dir: PathBuf,
    pub warning: Option<String>,
}

impl Persistence {
    pub fn load(dir: PathBuf) -> Self {
        let path = dir.join("state.json");
        let result = match fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 4 * 1024 * 1024 {
                    Err("数据文件超过 4 MB 限制".into())
                } else {
                    serde_json::from_slice::<Store>(&bytes)
                        .map_err(|e| e.to_string())
                        .and_then(|store| {
                            store.validate()?;
                            Ok(store)
                        })
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(error) => Err(error.to_string()),
        };
        match result {
            Ok(data) => Self { data, dir, warning: None },
            Err(error) => Self {
                data: Store::default(), dir,
                warning: Some(format!("无法读取 state.json，已进入只读模式以保护原文件。请备份数据目录并修复文件后重启。原因：{error}")),
            },
        }
    }

    pub fn ensure_writable(&self) -> AppResult<()> {
        if let Some(warning) = &self.warning {
            return Err(warning.clone());
        }
        Ok(())
    }

    pub fn commit(&mut self, next: Store) -> AppResult<()> {
        self.ensure_writable()?;
        next.validate()?;
        fs::create_dir_all(&self.dir).map_err(|e| format!("无法创建数据目录：{e}"))?;
        let path = self.dir.join("state.json");
        let temp = self.dir.join(format!("state-{}.tmp", Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let from: Vec<_> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let ok = unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(format!("无法安全保存数据，操作未提交：{error}"));
        }
        self.data = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reloads_atomically() {
        let dir = std::env::temp_dir().join(format!("mac-manager-test-{}", Uuid::new_v4()));
        let mut store = Persistence::load(dir.clone());
        let mut next = store.data.clone();
        next.settings.monitor = false;
        store.commit(next).unwrap();
        let loaded = Persistence::load(dir.clone());
        assert!(!loaded.data.settings.monitor);
        assert!(loaded.warning.is_none());
        let mut next = loaded.data;
        next.settings.monitor = true;
        store.commit(next).unwrap();
        assert!(Persistence::load(dir.clone()).data.settings.monitor);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corruption_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("mac-manager-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("state.json"), "broken").unwrap();
        let mut store = Persistence::load(dir.clone());
        assert!(store.warning.is_some());
        assert!(store.commit(Store::default()).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("state.json")).unwrap(),
            "broken"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
