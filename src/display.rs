//! Reading and changing the Windows desktop layout.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{LPARAM, POINTL, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CDS_NORESET, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_BADDUALVIEW,
    DISP_CHANGE_BADFLAGS, DISP_CHANGE_BADMODE, DISP_CHANGE_BADPARAM, DISP_CHANGE_FAILED,
    DISP_CHANGE_NOTUPDATED, DISP_CHANGE_RESTART, DISP_CHANGE_SUCCESSFUL, DISPLAY_DEVICE_ACTIVE,
    DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW,
    DM_POSITION, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW, EnumDisplayMonitors,
    EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

use crate::edid::{self, EdidInfo};

const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;

#[derive(Clone, Debug)]
pub struct Display {
    /// GDI device name, e.g. `\\.\DISPLAY2`.
    pub device_name: String,
    /// Number from the device name, the same one Windows shows in its settings.
    pub number: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub refresh: u32,
    /// Rotated by 90 or 270 degrees: the panel's width runs vertically.
    pub rotated: bool,
    pub primary: bool,
    pub dpi_scale: f32,
    pub edid: Option<EdidInfo>,
}

impl Display {
    /// Key under which user settings for this panel are stored.
    pub fn key(&self) -> String {
        match &self.edid {
            Some(e) => e.id.clone(),
            None => self.device_name.clone(),
        }
    }
}

pub fn enumerate() -> Vec<Display> {
    let dpis = monitor_scales();
    let mut out = Vec::new();
    unsafe {
        let mut i = 0;
        loop {
            let mut dd: DISPLAY_DEVICEW = zeroed();
            dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(null(), i, &mut dd, 0) == 0 {
                break;
            }
            i += 1;
            if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }

            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsW(dd.DeviceName.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) == 0 {
                continue;
            }
            let pos = dm.Anonymous1.Anonymous2.dmPosition;
            let orientation = dm.Anonymous1.Anonymous2.dmDisplayOrientation;

            let device_name = wstr(&dd.DeviceName);
            let (edid, monitor_string) = monitor_edid(&dd.DeviceName);
            let name = edid
                .as_ref()
                .and_then(|e| e.name.clone())
                .or(monitor_string)
                .unwrap_or_else(|| wstr(&dd.DeviceString));
            let dpi_scale = dpis
                .iter()
                .find(|(n, _)| *n == device_name)
                .map(|(_, s)| *s)
                .unwrap_or(1.0);

            out.push(Display {
                number: trailing_number(&device_name),
                device_name,
                name,
                x: pos.x,
                y: pos.y,
                width: dm.dmPelsWidth as i32,
                height: dm.dmPelsHeight as i32,
                refresh: dm.dmDisplayFrequency,
                rotated: orientation == 1 || orientation == 3,
                primary: dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
                dpi_scale,
                edid,
            });
        }
    }
    out.sort_by_key(|d| d.number);
    out
}

/// Finds the active monitor attached to an adapter output and reads its EDID from the registry.
unsafe fn monitor_edid(adapter: &[u16]) -> (Option<EdidInfo>, Option<String>) {
    let mut j = 0;
    loop {
        let mut md: DISPLAY_DEVICEW = unsafe { zeroed() };
        md.cb = size_of::<DISPLAY_DEVICEW>() as u32;
        if unsafe { EnumDisplayDevicesW(adapter.as_ptr(), j, &mut md, EDD_GET_DEVICE_INTERFACE_NAME) }
            == 0
        {
            return (None, None);
        }
        j += 1;
        if md.StateFlags & DISPLAY_DEVICE_ACTIVE == 0 {
            continue;
        }
        let monitor_string = Some(wstr(&md.DeviceString)).filter(|s| !s.is_empty());
        // \\?\DISPLAY#GSM5C19#5&34657b38&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}
        let id = wstr(&md.DeviceID);
        let parts: Vec<&str> = id.split('#').collect();
        if parts.len() < 3 {
            return (None, monitor_string);
        }
        let key = format!(
            r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{}\{}\Device Parameters",
            parts[1], parts[2]
        );
        let edid = read_registry_binary(&key, "EDID").and_then(|b| edid::parse(&b));
        return (edid, monitor_string);
    }
}

fn read_registry_binary(key: &str, value: &str) -> Option<Vec<u8>> {
    let key = wide(key);
    let value = wide(value);
    let mut buf = vec![0u8; 1024];
    let mut size = buf.len() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            null_mut(),
            buf.as_mut_ptr() as _,
            &mut size,
        )
    };
    if r != 0 {
        return None;
    }
    buf.truncate(size as usize);
    Some(buf)
}

fn monitor_scales() -> Vec<(String, f32)> {
    unsafe extern "system" fn callback(
        hm: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lp: LPARAM,
    ) -> windows_sys::core::BOOL {
        let list = unsafe { &mut *(lp as *mut Vec<(String, f32)>) };
        let mut mi: MONITORINFOEXW = unsafe { zeroed() };
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(hm, &mut mi as *mut _ as *mut MONITORINFO) } != 0 {
            let (mut dx, mut dy) = (96u32, 96u32);
            unsafe { GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            list.push((wstr(&mi.szDevice), dx as f32 / 96.0));
        }
        1
    }

    let mut list: Vec<(String, f32)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(null_mut(), null(), Some(callback), &mut list as *mut _ as LPARAM);
    }
    list
}

/// Moves displays to the given desktop positions in one go.
/// The primary display must be at (0, 0).
pub fn apply_positions(items: &[(String, i32, i32)]) -> Result<(), String> {
    unsafe {
        for (name, x, y) in items {
            let w = wide(name);
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsW(w.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) == 0 {
                return Err(format!("{name}: не удалось прочитать текущий режим"));
            }
            dm.dmFields = DM_POSITION;
            dm.Anonymous1.Anonymous2.dmPosition = POINTL { x: *x, y: *y };
            let r = ChangeDisplaySettingsExW(
                w.as_ptr(),
                &dm,
                null_mut(),
                CDS_UPDATEREGISTRY | CDS_NORESET,
                null(),
            );
            if r != DISP_CHANGE_SUCCESSFUL {
                return Err(format!("{name}: {}", change_error(r)));
            }
        }
        let r = ChangeDisplaySettingsExW(null(), null(), null_mut(), 0, null());
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(change_error(r).to_string());
        }
    }
    Ok(())
}

fn change_error(code: i32) -> &'static str {
    match code {
        DISP_CHANGE_BADDUALVIEW => "система не поддерживает такую раскладку",
        DISP_CHANGE_BADFLAGS => "неверные флаги",
        DISP_CHANGE_BADMODE => "режим не поддерживается",
        DISP_CHANGE_BADPARAM => "неверный параметр",
        DISP_CHANGE_FAILED => "драйвер отклонил изменение",
        DISP_CHANGE_NOTUPDATED => "не удалось записать настройки в реестр",
        DISP_CHANGE_RESTART => "нужна перезагрузка",
        _ => "неизвестная ошибка",
    }
}

fn trailing_number(s: &str) -> u32 {
    let digits: String = s.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
    digits.chars().rev().collect::<String>().parse().unwrap_or(0)
}

pub fn wstr(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
