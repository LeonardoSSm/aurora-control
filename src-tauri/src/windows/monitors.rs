//! Display monitors from Win32, and the choice of the one that holds the rail.
//!
//! The stable identifier is a digest of the monitor's device interface path
//! (which encodes the panel's hardware ID), so the preference survives
//! reboots and changes of the primary monitor without storing hardware
//! serials in clear text (audit I07).

use serde::Serialize;

use super::position::Rect;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    /// `m-` plus 16 hex digits.
    pub id: String,
    pub name: String,
    pub primary: bool,
    #[serde(skip)]
    pub work: Rect,
    /// Physical size, for display.
    pub width: i32,
    pub height: i32,
    /// Effective DPI scale (1.0 at 96 DPI).
    pub scale: f64,
}

/// FNV-1a: stable across runs and versions, unlike `DefaultHasher`.
pub fn stable_id(device: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device.to_ascii_uppercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("m-{hash:016x}")
}

/// The monitor for the rail: the preferred one when connected, otherwise the
/// primary (fallback, without forgetting the preference), otherwise any.
/// Returns the monitor and whether it is a fallback.
pub fn choose<'a>(monitors: &'a [Monitor], preferred: Option<&str>) -> Option<(&'a Monitor, bool)> {
    if let Some(wanted) = preferred
        && let Some(monitor) = monitors.iter().find(|monitor| monitor.id == wanted)
    {
        return Some((monitor, false));
    }
    let fallback = preferred.is_some();
    monitors
        .iter()
        .find(|monitor| monitor.primary)
        .or_else(|| monitors.first())
        .map(|monitor| (monitor, fallback))
}

#[cfg(windows)]
pub fn enumerate() -> Vec<Monitor> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
        MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    const MONITORINFOF_PRIMARY: u32 = 1;
    const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;

    fn text(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows_sys::core::BOOL {
        // SAFETY: `data` is the `Vec<HMONITOR>` passed below, alive for the call.
        unsafe { (*(data as *mut Vec<HMONITOR>)).push(monitor) };
        1
    }

    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into the vector owned by this frame.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut handles as *mut _ as LPARAM,
        );
    }

    handles
        .into_iter()
        .take(16)
        .filter_map(|handle| {
            // SAFETY: plain-data structs initialized with their size field.
            let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if unsafe { GetMonitorInfoW(handle, &mut info as *mut _ as *mut _) } == 0 {
                return None;
            }
            let device = text(&info.szDevice);
            let mut adapter_name = info.szDevice;
            let mut display: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
            display.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            // The first device attached to this output is the monitor itself.
            let found = unsafe {
                EnumDisplayDevicesW(
                    adapter_name.as_mut_ptr(),
                    0,
                    &mut display,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            } != 0;
            let interface = found
                .then(|| text(&display.DeviceID))
                .filter(|id| !id.is_empty());
            let friendly = found
                .then(|| text(&display.DeviceString))
                .filter(|name| !name.trim().is_empty());
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let scale =
                if unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
                    == 0
                {
                    f64::from(dpi_x) / 96.0
                } else {
                    1.0
                };
            let bounds = info.monitorInfo.rcMonitor;
            let work = info.monitorInfo.rcWork;
            Some(Monitor {
                id: stable_id(interface.as_deref().unwrap_or(&device)),
                name: friendly.unwrap_or_else(|| device.trim_start_matches(r"\\.\").to_string()),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                work: Rect {
                    x: work.left,
                    y: work.top,
                    width: work.right - work.left,
                    height: work.bottom - work.top,
                },
                width: bounds.right - bounds.left,
                height: bounds.bottom - bounds.top,
                scale,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: &str, primary: bool, x: i32, scale: f64) -> Monitor {
        Monitor {
            id: stable_id(id),
            name: id.into(),
            primary,
            work: Rect {
                x,
                y: 0,
                width: 1920,
                height: 1040,
            },
            width: 1920,
            height: 1080,
            scale,
        }
    }

    #[test]
    fn identifiers_are_stable_and_opaque() {
        let id = stable_id(r"\\?\DISPLAY#DEL4065#5&1a2b#0#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}");
        assert_eq!(
            id,
            stable_id(r"\\?\display#del4065#5&1a2b#0#{E6F07B5F-EE97-4A90-B076-33F57BF4EAA7}")
        );
        assert_eq!(id.len(), 18);
        assert!(!id.contains("DEL4065"));
        assert_ne!(id, stable_id(r"\\?\DISPLAY#SAM0F12#5&1a2b#0#{x}"));
    }

    #[test]
    fn i07_the_chosen_monitor_survives_a_primary_switch() {
        let monitors = [monitor("a", false, -1920, 1.5), monitor("b", true, 0, 1.0)];
        let (chosen, fallback) = choose(&monitors, Some(&stable_id("a"))).unwrap();
        assert_eq!((chosen.name.as_str(), fallback), ("a", false));
        // Primary moves to the other monitor: the choice still follows "a".
        let swapped = [monitor("a", true, 0, 1.5), monitor("b", false, 1920, 1.0)];
        assert_eq!(choose(&swapped, Some(&stable_id("a"))).unwrap().0.name, "a");
    }

    #[test]
    fn i07_a_disconnected_monitor_falls_back_without_losing_the_choice() {
        let monitors = [monitor("b", true, 0, 1.0), monitor("c", false, 1920, 1.25)];
        let wanted = stable_id("a");
        let (chosen, fallback) = choose(&monitors, Some(&wanted)).unwrap();
        assert_eq!((chosen.name.as_str(), fallback), ("b", true));
        // Reconnected: chosen again.
        let back = [monitor("b", true, 0, 1.0), monitor("a", false, -1920, 1.5)];
        assert_eq!(choose(&back, Some(&wanted)).unwrap(), (&back[1], false));
        // No primary flag (transient topology): any monitor rather than none.
        let odd = [monitor("c", false, 0, 1.0)];
        assert_eq!(choose(&odd, None).unwrap(), (&odd[0], false));
        assert_eq!(choose(&[], None), None);
    }
}
