mod display;
mod edid;

use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    for d in display::enumerate() {
        let size = d.edid.as_ref().and_then(|e| e.size_mm);
        println!(
            "{} {} {}x{} at ({}, {}) {:?} mm{}",
            d.number,
            d.name,
            d.width,
            d.height,
            d.x,
            d.y,
            size,
            if d.primary { " primary" } else { "" }
        );
    }
}
