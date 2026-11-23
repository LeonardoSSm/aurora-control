//! Optional start with Windows, through the per-user `Run` key.
//!
//! Only the value named "Aurora Control" under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` is read, written or
//! deleted, and only when the user changes the setting. No administrator
//! rights, services or scheduled tasks are involved. If the user disables the
//! entry in Task Manager, Windows keeps that choice in a separate key that
//! this app never touches.

use std::io;
use std::path::Path;

use serde::Serialize;

const VALUE: &str = "Aurora Control";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Enabled,
    Disabled,
    /// An entry exists for another copy of the app (moved or reinstalled).
    OtherLocation,
    /// The registry could not be read.
    Unknown,
}

/// The value this app manages. A seam for tests.
pub trait RunKey {
    fn read(&self) -> io::Result<Option<String>>;
    fn write(&self, command: &str) -> io::Result<()>;
    fn delete(&self) -> io::Result<()>;
}

pub fn command_line(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

pub fn status(key: &impl RunKey, exe: &Path) -> Status {
    match key.read() {
        Ok(None) => Status::Disabled,
        Ok(Some(command)) if command.eq_ignore_ascii_case(&command_line(exe)) => Status::Enabled,
        Ok(Some(_)) => Status::OtherLocation,
        Err(_) => Status::Unknown,
    }
}

/// Enables or disables the entry for `exe`. Idempotent.
pub fn apply(key: &impl RunKey, exe: &Path, enabled: bool) -> io::Result<()> {
    if enabled {
        key.write(&command_line(exe))
    } else {
        match key.read()? {
            Some(_) => key.delete(),
            None => Ok(()),
        }
    }
}

/// Applies the start-up choice, then runs `persist` (saving the
/// preferences). If `persist` fails, the entry is restored exactly as it was
/// before (including an entry of another copy), so the registry and the saved
/// preference never disagree.
pub fn apply_then<T>(
    key: &impl RunKey,
    exe: &Path,
    enabled: bool,
    persist: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let before = key
        .read()
        .map_err(|error| format!("Inicialização com o Windows: {error}"))?;
    apply(key, exe, enabled).map_err(|error| format!("Inicialização com o Windows: {error}"))?;
    persist().inspect_err(|_| {
        let restored = match &before {
            Some(command) => key.write(command),
            None => key.delete(),
        };
        if restored.is_err() {
            eprintln!("Aurora Control: could not restore the start-up entry");
        }
    })
}

#[cfg(windows)]
pub struct Registry {
    subkey: String,
}

#[cfg(windows)]
impl Registry {
    /// The per-user `Run` key.
    pub fn run() -> Self {
        Self {
            subkey: RUN_SUBKEY.into(),
        }
    }
}

#[cfg(windows)]
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
mod registry {
    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    /// Longest command accepted back from the registry, in UTF-16 units.
    const MAX_COMMAND: usize = 2048;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn error(code: u32) -> io::Error {
        io::Error::from_raw_os_error(code as i32)
    }

    impl RunKey for Registry {
        fn read(&self) -> io::Result<Option<String>> {
            let (subkey, value) = (wide(&self.subkey), wide(VALUE));
            let mut buffer = vec![0u16; MAX_COMMAND];
            let mut size = (buffer.len() * 2) as u32;
            // SAFETY: the buffer and size describe owned, writable memory.
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    let units = (size as usize / 2).min(buffer.len());
                    let end = buffer[..units]
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(units);
                    Ok(Some(String::from_utf16_lossy(&buffer[..end])))
                }
                ERROR_FILE_NOT_FOUND => Ok(None),
                ERROR_MORE_DATA => Ok(Some(String::new())),
                code => Err(error(code)),
            }
        }

        fn write(&self, command: &str) -> io::Result<()> {
            let (subkey, value, data) = (wide(&self.subkey), wide(VALUE), wide(command));
            // SAFETY: all pointers reference owned, NUL-terminated buffers.
            let status = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            if status == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(error(status))
            }
        }

        fn delete(&self) -> io::Result<()> {
            let (subkey, value) = (wide(&self.subkey), wide(VALUE));
            // SAFETY: both pointers reference owned, NUL-terminated buffers.
            let status =
                unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), value.as_ptr()) };
            match status {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
                code => Err(error(code)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        value: RefCell<Option<String>>,
        broken: bool,
    }

    impl RunKey for Fake {
        fn read(&self) -> io::Result<Option<String>> {
            if self.broken {
                return Err(io::Error::other("denied"));
            }
            Ok(self.value.borrow().clone())
        }
        fn write(&self, command: &str) -> io::Result<()> {
            *self.value.borrow_mut() = Some(command.into());
            Ok(())
        }
        fn delete(&self) -> io::Result<()> {
            *self.value.borrow_mut() = None;
            Ok(())
        }
    }

    #[test]
    fn enabling_and_disabling_is_idempotent_and_quoted() {
        let key = Fake::default();
        let exe = Path::new(r"C:\Users\Ana Clara\AppData\Local\Aurora Control\aurora-control.exe");
        assert_eq!(status(&key, exe), Status::Disabled);
        apply(&key, exe, true).unwrap();
        apply(&key, exe, true).unwrap();
        assert_eq!(
            key.value.borrow().as_deref(),
            Some(r#""C:\Users\Ana Clara\AppData\Local\Aurora Control\aurora-control.exe""#)
        );
        assert_eq!(status(&key, exe), Status::Enabled);
        apply(&key, exe, false).unwrap();
        apply(&key, exe, false).unwrap();
        assert_eq!(status(&key, exe), Status::Disabled);
    }

    #[test]
    fn a_failed_save_restores_the_previous_entry() {
        let exe = Path::new(r"C:\new\aurora-control.exe");
        let failing = || Err::<(), _>("disk full".to_string());
        // Enabling from nothing: the new entry is removed again.
        let key = Fake::default();
        assert!(apply_then(&key, exe, true, failing).is_err());
        assert_eq!(status(&key, exe), Status::Disabled);
        // Disabling an enabled entry: it comes back.
        apply(&key, exe, true).unwrap();
        assert!(apply_then(&key, exe, false, failing).is_err());
        assert_eq!(status(&key, exe), Status::Enabled);
        // Another copy's entry is restored verbatim.
        let other = r#""D:\old\aurora-control.exe""#;
        key.write(other).unwrap();
        assert!(apply_then(&key, exe, true, failing).is_err());
        assert_eq!(key.value.borrow().as_deref(), Some(other));
        // A successful save keeps the change.
        assert_eq!(apply_then(&key, exe, true, || Ok(7)), Ok(7));
        assert_eq!(status(&key, exe), Status::Enabled);
    }

    #[test]
    fn an_entry_for_another_copy_is_reported_and_replaced_on_enable() {
        let key = Fake::default();
        key.write(r#""D:\old\aurora-control.exe""#).unwrap();
        let exe = Path::new(r"C:\new\aurora-control.exe");
        assert_eq!(status(&key, exe), Status::OtherLocation);
        apply(&key, exe, true).unwrap();
        assert_eq!(status(&key, exe), Status::Enabled);
        let broken = Fake {
            broken: true,
            ..Fake::default()
        };
        assert_eq!(status(&broken, exe), Status::Unknown);
        assert!(apply(&broken, exe, false).is_err());
    }
}

/// Exercises the real registry wrapper on a scratch key, never on `Run`.
/// Ignored by default because it writes to HKCU; run with
/// `cargo test -- --ignored registry_round_trip`.
#[cfg(all(test, windows))]
mod registry_tests {
    use super::*;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW};

    #[test]
    #[ignore = "writes a scratch key under HKCU"]
    fn registry_round_trip() {
        let subkey = format!(r"Software\AuroraControlTest-{}", std::process::id());
        let key = Registry {
            subkey: subkey.clone(),
        };
        let exe = Path::new(r"C:\Program Files\Aurora Control\aurora-control.exe");
        assert_eq!(status(&key, exe), Status::Disabled);
        apply(&key, exe, true).unwrap();
        assert_eq!(status(&key, exe), Status::Enabled);
        assert_eq!(
            key.read().unwrap().as_deref(),
            Some(r#""C:\Program Files\Aurora Control\aurora-control.exe""#)
        );
        apply(&key, exe, false).unwrap();
        assert_eq!(status(&key, exe), Status::Disabled);
        apply(&key, exe, false).unwrap();
        let wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: the name is NUL-terminated; the scratch key is now empty.
        assert_eq!(
            unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide.as_ptr()) },
            0
        );
    }
}
