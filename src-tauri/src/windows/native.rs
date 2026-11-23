use std::io;
use tauri::WebviewWindow;
use windows_sys::Win32::{
    Foundation::{HWND, SetLastError},
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, IsWindowVisible, SWP_HIDEWINDOW,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
        SetWindowLongPtrW, SetWindowPos, WS_EX_NOACTIVATE,
    },
};

use super::position::Rect;

fn hwnd(window: &WebviewWindow) -> tauri::Result<HWND> {
    Ok(window.hwnd()?.0)
}

pub(super) fn prevent_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let handle = hwnd(window)?;
    // SAFETY: Tauri owns the live HWND. This changes only the extended window style.
    unsafe {
        let current = GetWindowLongPtrW(handle, GWL_EXSTYLE);
        SetLastError(0);
        if SetWindowLongPtrW(handle, GWL_EXSTYLE, current | WS_EX_NOACTIVATE as isize) == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

pub(super) fn is_visible(window: &WebviewWindow) -> tauri::Result<bool> {
    // SAFETY: The HWND was obtained from the live window owned by Tauri.
    Ok(unsafe { IsWindowVisible(hwnd(window)?) } != 0)
}

pub(super) fn place(window: &WebviewWindow, rect: Rect, show: bool) -> tauri::Result<()> {
    let mut flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    if show {
        flags |= SWP_SHOWWINDOW;
    }

    // SAFETY: The HWND is live, and SetWindowPos does not retain any Rust data.
    let success = unsafe {
        SetWindowPos(
            hwnd(window)?,
            HWND_TOPMOST,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            flags,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) fn hide(window: &WebviewWindow) -> tauri::Result<()> {
    // SAFETY: The HWND is live. Only its visibility changes.
    let success = unsafe {
        SetWindowPos(
            hwnd(window)?,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
