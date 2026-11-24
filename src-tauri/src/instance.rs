//! Single instance per Windows session (audit M04).
//!
//! A named mutex in the session namespace is taken before any worker starts.
//! A second launch finds it, exits immediately and never queries the
//! providers, so two processes cannot double the request rate or ignore each
//! other's `Retry-After`.

use std::io;

#[cfg(windows)]
pub struct Instance(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: the handle is only closed on drop; it is never used concurrently.
#[cfg(windows)]
unsafe impl Send for Instance {}
#[cfg(windows)]
unsafe impl Sync for Instance {}

#[cfg(windows)]
impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the mutex handle.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Claims `name`. `Ok(None)` when another live instance holds it.
#[cfg(windows)]
pub fn claim(name: &str) -> io::Result<Option<Instance>> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let wide: Vec<u16> = format!("Local\\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: the name is NUL-terminated and outlives the call.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: reads the error code of the call above, on this thread.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: our handle to the existing mutex; the owner keeps its own.
        unsafe { CloseHandle(handle) };
        return Ok(None);
    }
    Ok(Some(Instance(handle)))
}

pub const NAME: &str = "app.auroracontrol.desktop.instance";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_is_refused_until_the_first_is_released() {
        let name = format!("aurora-control-test-{}", std::process::id());
        let first = claim(&name).unwrap();
        assert!(first.is_some());
        assert!(claim(&name).unwrap().is_none(), "second instance refused");
        drop(first);
        assert!(claim(&name).unwrap().is_some(), "free again after exit");
    }
}
