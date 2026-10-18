//! Millimetre rulers drawn on the real screens along every shared edge.
//!
//! Each ruler is a small click-through topmost window that lives on its own thread with a plain
//! Win32 message loop, so it keeps exact physical pixels regardless of what the UI toolkit does.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, mpsc};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DeleteDC, DeleteObject, EndPaint,
    FW_NORMAL, FW_SEMIBOLD, FillRect, GetTextExtentPoint32W, HDC, HFONT, InvalidateRect, LineTo,
    MoveToEx, PAINTSTRUCT, PS_SOLID, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    TextOutW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    LWA_ALPHA, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassExW,
    SW_SHOWNOACTIVATE, SetLayeredWindowAttributes, SetTimer, ShowWindow, TranslateMessage,
    WM_APP, WM_ERASEBKGND, WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::display::wide;
use crate::layout::{Anchor, Axis, Edge, Mon, Rect, anchor_t};

/// What the rulers should show: the layout currently applied in Windows.
#[derive(Clone, Default)]
pub struct OverlayData {
    pub mons: Vec<Mon>,
    /// Short label of each monitor, e.g. `2`.
    pub labels: Vec<String>,
    pub scales: Vec<f32>,
    pub edges: Vec<Edge>,
    pub anchor: Anchor,
}

const WM_APP_REBUILD: u32 = WM_APP + 1;
const HTTRANSPARENT: LRESULT = -1;

pub struct Overlay {
    shared: Arc<Mutex<Option<OverlayData>>>,
    thread_id: u32,
}

impl Overlay {
    pub fn start() -> Self {
        let shared = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let thread_shared = shared.clone();
        std::thread::spawn(move || unsafe { run(thread_shared, tx) });
        let thread_id = rx.recv().unwrap_or(0);
        Self { shared, thread_id }
    }

    /// Replaces the rulers; pass `OverlayData::default()` to hide them.
    pub fn set(&self, data: OverlayData) {
        *self.shared.lock().unwrap() = Some(data);
        unsafe { PostThreadMessageW(self.thread_id, WM_APP_REBUILD, 0, 0) };
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
    }
}

/// Which side of its monitor a ruler sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

struct Strip {
    hwnd: HWND,
    mon: usize,
    edge: usize,
    side: Side,
    /// Window rectangle in desktop pixels.
    rect: Rect,
}

#[derive(Default)]
struct State {
    data: OverlayData,
    strips: Vec<Strip>,
    cursor: (i32, i32),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

unsafe fn run(shared: Arc<Mutex<Option<OverlayData>>>, tx: mpsc::Sender<u32>) {
    unsafe {
        let mut msg: MSG = zeroed();
        // Make sure the thread has a message queue before anyone posts to it.
        PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
        let _ = tx.send(GetCurrentThreadId());

        let hinst = GetModuleHandleW(null());
        let class = wide("MonitorAlignerRuler");
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinst;
        wc.lpszClassName = class.as_ptr();
        RegisterClassExW(&wc);

        SetTimer(null_mut(), 0, 33, None);

        loop {
            let r = GetMessageW(&mut msg, null_mut(), 0, 0);
            if r == 0 || r == -1 {
                break;
            }
            if msg.hwnd.is_null() {
                match msg.message {
                    WM_APP_REBUILD => {
                        let data = shared.lock().unwrap().take();
                        if let Some(data) = data {
                            rebuild(data, hinst, &class);
                        }
                    }
                    WM_TIMER => tick(),
                    _ => {}
                }
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        rebuild(OverlayData::default(), hinst, &class);
    }
}

unsafe fn rebuild(data: OverlayData, hinst: *mut core::ffi::c_void, class: &[u16]) {
    let old: Vec<HWND> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        std::mem::take(&mut s.strips).into_iter().map(|st| st.hwnd).collect()
    });
    for h in old {
        unsafe { DestroyWindow(h) };
    }

    let mut strips = layout_strips(&data);
    let title = wide("Ruler");
    for st in &mut strips {
        let r = st.rect;
        st.hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                r.x,
                r.y,
                r.w,
                r.h,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            )
        };
        if !st.hwnd.is_null() {
            unsafe { SetLayeredWindowAttributes(st.hwnd, 0, 235, LWA_ALPHA) };
        }
    }
    let hwnds: Vec<HWND> = strips.iter().map(|s| s.hwnd).filter(|h| !h.is_null()).collect();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.data = data;
        s.strips = strips;
    });
    for h in hwnds {
        unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
    }
}

fn layout_strips(d: &OverlayData) -> Vec<Strip> {
    let mut out = Vec::new();
    for (ei, e) in d.edges.iter().enumerate() {
        for (mi, side) in match e.axis {
            Axis::Vertical => [(e.a, Side::Right), (e.b, Side::Left)],
            Axis::Horizontal => [(e.a, Side::Bottom), (e.b, Side::Top)],
        } {
            let depth = (96.0 * d.scales.get(mi).copied().unwrap_or(1.0)).round() as i32;
            let m = &d.mons[mi].rect;
            let depth = depth.min(match e.axis {
                Axis::Vertical => m.w / 3,
                Axis::Horizontal => m.h / 3,
            });
            let len = e.s1 - e.s0;
            let rect = match side {
                Side::Right => Rect { x: e.pos - depth, y: e.s0, w: depth, h: len },
                Side::Left => Rect { x: e.pos, y: e.s0, w: depth, h: len },
                Side::Bottom => Rect { x: e.s0, y: e.pos - depth, w: len, h: depth },
                Side::Top => Rect { x: e.s0, y: e.pos, w: len, h: depth },
            };
            out.push(Strip { hwnd: null_mut(), mon: mi, edge: ei, side, rect });
        }
    }
    out
}

unsafe fn tick() {
    let mut p: POINT = unsafe { zeroed() };
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return;
    }
    let hwnds: Vec<HWND> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.cursor == (p.x, p.y) {
            return Vec::new();
        }
        s.cursor = (p.x, p.y);
        s.strips.iter().map(|st| st.hwnd).collect()
    });
    for h in hwnds {
        unsafe { InvalidateRect(h, null(), 0) };
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = zeroed();
                let hdc = BeginPaint(h, &mut ps);
                STATE.with(|s| {
                    if let Ok(s) = s.try_borrow() {
                        if let Some(st) = s.strips.iter().find(|st| st.hwnd == h) {
                            paint(hdc, st, &s);
                        }
                    }
                });
                EndPaint(h, &ps);
                0
            }
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => HTTRANSPARENT,
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

const BG: u32 = rgb(20, 22, 28);
const EDGE: u32 = rgb(60, 200, 255);
const TICK: u32 = rgb(235, 235, 235);
const TICK_MINOR: u32 = rgb(150, 150, 160);
const ZERO: u32 = rgb(255, 70, 70);
const CURSOR: u32 = rgb(255, 210, 40);
const LABEL_BG: u32 = rgb(40, 44, 56);

unsafe fn line(hdc: HDC, color: u32, width: i32, a: (i32, i32), b: (i32, i32)) {
    unsafe {
        let pen = CreatePen(PS_SOLID, width, color);
        let old = SelectObject(hdc, pen);
        MoveToEx(hdc, a.0, a.1, null_mut());
        LineTo(hdc, b.0, b.1);
        SelectObject(hdc, old);
        DeleteObject(pen);
    }
}

unsafe fn fill(hdc: HDC, color: u32, r: RECT) {
    unsafe {
        let br = CreateSolidBrush(color);
        FillRect(hdc, &r, br);
        DeleteObject(br);
    }
}

unsafe fn text_size(hdc: HDC, s: &[u16]) -> (i32, i32) {
    let mut sz: SIZE = unsafe { zeroed() };
    unsafe { GetTextExtentPoint32W(hdc, s.as_ptr(), s.len() as i32, &mut sz) };
    (sz.cx, sz.cy)
}

unsafe fn font(height: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as _,
            0,
            0,
            ANTIALIASED_QUALITY as _,
            0,
            face.as_ptr(),
        )
    }
}

unsafe fn paint(hdc: HDC, st: &Strip, s: &State) {
    let r = st.rect;
    let (w, h) = (r.w, r.h);
    let d = &s.data;
    let (Some(m), Some(e)) = (d.mons.get(st.mon), d.edges.get(st.edge)) else { return };
    let scale = d.scales.get(st.mon).copied().unwrap_or(1.0);
    let px = |v: f32| (v * scale).round() as i32;

    let vertical = e.axis == Axis::Vertical;
    let ppmm = e.ppmm(m);
    // Desktop coordinate of local position 0 along the ruler, and the ruler's length and depth.
    let origin = if vertical { r.y } else { r.x } as f64;
    let len = if vertical { h } else { w };
    let depth = if vertical { w } else { h };
    // (position along the edge, distance from the edge) -> local window pixel.
    let to_xy = |a: i32, dist: i32| -> (i32, i32) {
        match st.side {
            Side::Right => (w - 1 - dist, a),
            Side::Left => (dist, a),
            Side::Bottom => (a, h - 1 - dist),
            Side::Top => (a, dist),
        }
    };

    unsafe {
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        fill(mem, BG, RECT { left: 0, top: 0, right: w, bottom: h });

        let small = font(px(12.0), FW_NORMAL as i32);
        let bold = font(px(13.0), FW_SEMIBOLD as i32);
        let old_font = SelectObject(mem, small);
        SetBkMode(mem, TRANSPARENT as _);

        // Draws text next to a point at `dist` from the edge, centred on position `a` along it.
        let label = |text: &str, a: i32, dist: i32, color: u32, boxed: bool| {
            let t: Vec<u16> = text.encode_utf16().collect();
            let (tw, th) = text_size(mem, &t);
            let (x, y) = to_xy(a, dist);
            let (x, y) = match st.side {
                Side::Right => (x - tw, y - th / 2),
                Side::Left => (x, y - th / 2),
                Side::Bottom => (x - tw / 2, y - th),
                Side::Top => (x - tw / 2, y),
            };
            if boxed {
                fill(mem, LABEL_BG, RECT { left: x - 3, top: y, right: x + tw + 3, bottom: y + th });
            }
            SetTextColor(mem, color);
            TextOutW(mem, x, y, t.as_ptr(), t.len() as i32);
        };

        let t0 = anchor_t(e, d.anchor, Some(s.cursor));
        let lo = ((origin - t0) / ppmm).floor() as i64 - 1;
        let hi = ((origin + len as f64 - t0) / ppmm).ceil() as i64 + 1;
        let fine = ppmm >= 2.5;
        let label_dist = px(34.0);
        for l in lo..=hi {
            let a = (t0 + l as f64 * ppmm - origin).round() as i32;
            if a < 0 || a >= len {
                continue;
            }
            let (tick_len, color) = if l == 0 {
                continue;
            } else if l % 10 == 0 {
                (px(28.0), TICK)
            } else if l % 5 == 0 {
                (px(18.0), TICK)
            } else if fine {
                (px(9.0), TICK_MINOR)
            } else {
                continue;
            };
            line(mem, color, 1, to_xy(a, 0), to_xy(a, tick_len));
            // Keep the top of vertical rulers free for the neighbour's number.
            if l % 10 == 0 && !(vertical && a < px(26.0)) {
                label(&l.to_string(), a, label_dist, TICK, false);
            }
        }

        // The zero: on both sides of the edge it marks the same physical height when aligned.
        let a0 = (t0 - origin).round() as i32;
        if (0..len).contains(&a0) {
            line(mem, ZERO, px(2.0).max(1), to_xy(a0, 0), to_xy(a0, depth));
            SelectObject(mem, bold);
            label("0", a0, label_dist, ZERO, true);
            SelectObject(mem, small);
        }

        // Where the cursor would cross the edge, in millimetres from the zero on this panel.
        let tc = e.along(s.cursor) as f64;
        let ac = (tc - origin).round() as i32;
        if (0..len).contains(&ac) {
            line(mem, CURSOR, 1, to_xy(ac, 0), to_xy(ac, depth));
            let mm = (tc - t0) / ppmm;
            let shift = if vertical { px(10.0) } else { 0 };
            label(&format!("{mm:+.1}"), ac + shift, px(56.0), CURSOR, true);
        }

        // The edge itself and the neighbour's number.
        line(mem, EDGE, px(3.0).max(2), to_xy(0, 0), to_xy(len, 0));
        let other = d.labels.get(e.other(st.mon)).cloned().unwrap_or_default();
        SelectObject(mem, bold);
        let head_at = if vertical { px(12.0) } else { px(40.0) };
        label(&format!("→ {other}"), head_at, px(56.0), EDGE, true);

        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);

        SelectObject(mem, old_font);
        DeleteObject(small);
        DeleteObject(bold);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
    }
}
