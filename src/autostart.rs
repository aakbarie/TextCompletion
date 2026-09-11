//! Launch Scriblet when the user signs in.
//!
//! - Windows: `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
//! - macOS: a LaunchAgent plist in `~/Library/LaunchAgents`
//! - elsewhere: unsupported

use anyhow::Result;

pub const REGISTRATION_NAME: &str = "Scriblet";

pub fn supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
}

pub fn is_enabled() -> Result<bool> {
    imp::is_enabled()
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    imp::set_enabled(enabled)
}

fn current_exe() -> Result<std::path::PathBuf> {
    std::env::current_exe()
        .map_err(|error| anyhow::anyhow!("cannot locate Scriblet executable: {error}"))
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
        value
            .as_ref()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    struct RunKey(HKEY);

    impl RunKey {
        fn open(access: u32) -> Result<Self> {
            let mut handle: HKEY = null_mut();
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    wide(RUN_KEY).as_ptr(),
                    0,
                    access,
                    &mut handle,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(anyhow::anyhow!(
                    "cannot open Run registry key (error {status})"
                ));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for RunKey {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }

    pub fn is_enabled() -> Result<bool> {
        let key = RunKey::open(KEY_QUERY_VALUE)?;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                wide(REGISTRATION_NAME).as_ptr(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        match status {
            ERROR_SUCCESS => Ok(true),
            ERROR_FILE_NOT_FOUND => Ok(false),
            other => Err(anyhow::anyhow!(
                "cannot read Run registry value (error {other})"
            )),
        }
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        let key = RunKey::open(KEY_SET_VALUE)?;
        let name = wide(REGISTRATION_NAME);
        if enabled {
            let command = format!("\"{}\"", current_exe()?.display());
            let data = wide(command);
            let bytes = (data.len() * std::mem::size_of::<u16>()) as u32;
            let status = unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    bytes,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(anyhow::anyhow!(
                    "cannot write Run registry value (error {status})"
                ));
            }
        } else {
            let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(anyhow::anyhow!(
                    "cannot remove Run registry value (error {status})"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    const LABEL: &str = "com.aakbarie.scriblet";

    fn plist_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?;
        Ok(dirs
            .home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    pub fn is_enabled() -> Result<bool> {
        Ok(plist_path()?.exists())
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        let path = plist_path()?;
        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let exe = current_exe()?;
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
                xml_escape(&exe.display().to_string())
            );
            std::fs::write(&path, plist)?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn xml_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use super::*;

    pub fn is_enabled() -> Result<bool> {
        Ok(false)
    }

    pub fn set_enabled(_enabled: bool) -> Result<()> {
        let _ = current_exe;
        Err(anyhow::anyhow!(
            "start at login is not supported on this platform"
        ))
    }
}
