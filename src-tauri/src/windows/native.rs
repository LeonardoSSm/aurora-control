use std::io;
use tauri::WebviewWindow;
use windows_sys::Win32::{
    Foundation::{HWND, POINT, SetLastError},
    Graphics::Gdi::{
        CombineRgn, CreateEllipticRgn, CreateRectRgn, DeleteObject, HRGN, RGN_AND, RGN_OR,
        SetWindowRgn,
    },
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, HWND_TOPMOST, IsIconic, IsWindowVisible,
        SW_SHOWNOACTIVATE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
        SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    },
};

/// Cursor position in physical screen pixels. The process is per-monitor DPI
/// aware, so these match the coordinates used by `SetWindowPos`.
pub(super) fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos writes into the provided POINT only.
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

use super::position::Rect;

fn hwnd(window: &WebviewWindow) -> tauri::Result<HWND> {
    Ok(window.hwnd()?.0)
}

/// Overlay styles: never activated, and a tool window, so it stays out of
/// Alt+Tab and "Show desktop" (Win+D) does not minimize it.
pub(super) fn make_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    let handle = hwnd(window)?;
    // SAFETY: Tauri owns the live HWND. This changes only the extended window style.
    unsafe {
        let current = GetWindowLongPtrW(handle, GWL_EXSTYLE);
        let style = (current | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize)
            & !(WS_EX_APPWINDOW as isize);
        SetLastError(0);
        if SetWindowLongPtrW(handle, GWL_EXSTYLE, style) == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

/// Shown and not minimized.
pub(super) fn is_visible(window: &WebviewWindow) -> tauri::Result<bool> {
    let handle = hwnd(window)?;
    // SAFETY: The HWND was obtained from the live window owned by Tauri.
    Ok(unsafe { IsWindowVisible(handle) != 0 && IsIconic(handle) == 0 })
}

pub(super) fn is_minimized(window: &WebviewWindow) -> tauri::Result<bool> {
    // SAFETY: The HWND was obtained from the live window owned by Tauri.
    Ok(unsafe { IsIconic(hwnd(window)?) } != 0)
}

struct Region(HRGN);

impl Region {
    fn new(handle: HRGN) -> io::Result<Self> {
        if handle.is_null() {
            return Err(io::Error::other(
                "Could not allocate the rail window region",
            ));
        }
        Ok(Self(handle))
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns the region handle.
        unsafe { DeleteObject(self.0) };
    }
}

pub(super) fn set_indicator_regions(
    window: &WebviewWindow,
    rect: Rect,
    circles: &[Rect],
) -> tauri::Result<()> {
    let handle = hwnd(window)?;
    let failed = || io::Error::other("Could not apply the rail window region").into();
    // SAFETY: Tauri owns the live HWND. GDI regions are owned by their wrappers
    // until the result is transferred to Windows by SetWindowRgn.
    unsafe {
        let ellipse =
            |r: Rect| Region::new(CreateEllipticRgn(r.x, r.y, r.x + r.width, r.y + r.height));
        let combined = Region::new(CreateRectRgn(0, 0, 0, 0))?;
        for circle in circles {
            let circle = ellipse(*circle)?;
            if CombineRgn(combined.0, combined.0, circle.0, RGN_OR) == 0 {
                return Err(failed());
            }
        }
        let area = Region::new(CreateRectRgn(0, 0, rect.width, rect.height))?;
        if CombineRgn(combined.0, combined.0, area.0, RGN_AND) == 0
            || SetWindowRgn(handle, combined.0, IsWindowVisible(handle)) == 0
        {
            return Err(failed());
        }
        std::mem::forget(combined);
    }
    Ok(())
}

pub(super) fn place(window: &WebviewWindow, rect: Rect, show: bool) -> tauri::Result<()> {
    let handle = hwnd(window)?;
    let mut flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    if show {
        flags |= SWP_SHOWWINDOW;
        // SWP_SHOWWINDOW does not restore a minimized window. Restore it
        // without activation first, or it would stay off screen.
        // SAFETY: The HWND is live. Only its show state changes.
        unsafe {
            if IsIconic(handle) != 0 {
                ShowWindow(handle, SW_SHOWNOACTIVATE);
            }
        }
    }

    // SAFETY: The HWND is live, and SetWindowPos does not retain any Rust data.
    let success = unsafe {
        SetWindowPos(
            handle,
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
