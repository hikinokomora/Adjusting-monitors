mod config;
mod display;
mod edid;
mod layout;

use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use layout::{Anchor, Mon, Rect, anchor_t, find_edges, implied_offset_mm};

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let config = config::Config::load();
    let displays = display::enumerate();
    let mons: Vec<Mon> = displays
        .iter()
        .map(|d| {
            let (w, h) = config
                .sizes
                .get(&d.key())
                .map(|s| (s[0], s[1]))
                .or(d.edid.as_ref().and_then(|e| e.size_mm))
                .unwrap_or((d.width as f32 * 0.27, d.height as f32 * 0.27));
            let (mm_w, mm_h) = if d.rotated { (h, w) } else { (w, h) };
            Mon { rect: Rect { x: d.x, y: d.y, w: d.width, h: d.height }, mm_w, mm_h }
        })
        .collect();
    for (d, m) in displays.iter().zip(&mons) {
        println!("{} {} {:?} {:.0}x{:.0} mm", d.number, d.name, m.rect, m.mm_w, m.mm_h);
    }
    for e in find_edges(&mons) {
        let t0 = anchor_t(&e, Anchor::Center, None);
        println!(
            "{} | {}: {:+.1} mm",
            displays[e.a].number,
            displays[e.b].number,
            implied_offset_mm(&e, &mons, t0)
        );
    }
}
