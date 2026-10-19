//! Millimetre rulers drawn on the real screens along every shared edge.
//!
//! Each ruler is a small topmost window that never takes focus. It lives on its own thread with
//! a plain Win32 message loop, so it keeps exact physical pixels whatever the UI toolkit does.
//! A ruler can be dragged or scrolled with the mouse wheel to shift its monitor along the edge.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, mpsc};

use eframe::egui;
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
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    HWND_TOPMOST, IDC_SIZENS, IDC_SIZEWE, LWA_ALPHA, LoadCursorW, MA_NOACTIVATE, MSG,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassExW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetCursor, SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, WM_APP, WM_CAPTURECHANGED, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_QUIT, WM_SETCURSOR, WM_TIMER,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::display::wide;
use crate::layout::{Anchor, Axis, Edge, Mon, Rect, anchor_t};

/// What the rulers should show.
#[derive(Clone, Default)]
pub struct OverlayData {
    /// Monitors where Windows has them now: this is where the ruler windows are placed.
    pub applied: Vec<Mon>,
    /// Monitors where the user has put them, maybe not applied yet: this is what the ticks show.
    pub edited: Vec<Mon>,
    /// Short label of each monitor, e.g. `2`.
    pub labels: Vec<String>,
    pub scales: Vec<f32>,
    /// Shared edges of the applied layout.
    pub edges: Vec<Edge>,
    pub anchor: Anchor,
}

/// Something the user did with a ruler.
pub enum OverlayEvent {
    /// Move a monitor by this many desktop pixels.
    Shift { mon: usize, dx: i32, dy: i32 },
    /// Windows reported a change of the desktop layout or resolution.
    DisplayChanged,
}

const WM_APP_REBUILD: u32 = WM_APP + 1;
const MK_SHIFT: usize = 0x0004;

pub struct Overlay {
    shared: Arc<Mutex<Option<OverlayData>>>,
    thread_id: u32,
}

impl Overlay {
    pub fn start(ctx: egui::Context) -> (Self, mpsc::Receiver<OverlayEvent>) {
        let shared = Arc::new(Mutex::new(None));
        let (tid_tx, tid_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let thread_shared = shared.clone();
        std::thread::spawn(move || unsafe { run(thread_shared, tid_tx, ev_tx, ctx) });
        let thread_id = tid_rx.recv().unwrap_or(0);
        (Self { shared, thread_id }, ev_rx)
    }

    /// Replaces what the rulers show; pass `OverlayData::default()` to hide them.
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

struct Drag {
    mon: usize,
    axis: Axis,
    start: (i32, i32),
    /// Pointer movement along the edge since the press, in pixels.
    delta: i32,
    /// Zero of the rulers frozen at the press, so the dragged ruler follows the pointer 1:1.
    t0: Vec<f64>,
}

struct State {
    data: OverlayData,
    strips: Vec<Strip>,
    cursor: (i32, i32),
    hovered: HWND,
    drag: Option<Drag>,
    /// Shift already sent to the app but not yet reflected in `data`.
    pending: Option<(usize, i32, i32)>,
    events: Option<(mpsc::Sender<OverlayEvent>, egui::Context)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            data: OverlayData::default(),
            strips: Vec::new(),
            cursor: (i32::MIN, i32::MIN),
            hovered: null_mut(),
            drag: None,
            pending: None,
            events: None,
        }
    }
}

impl State {
    /// Edited monitor position including a drag in progress and shifts not yet confirmed.
    fn preview(&self, i: usize) -> Mon {
        let mut m = self.data.edited[i];
        if let Some((mon, dx, dy)) = self.pending {
            if mon == i {
                m.rect.x += dx;
                m.rect.y += dy;
            }
        }
        if let Some(d) = &self.drag {
            if d.mon == i {
                match d.axis {
                    Axis::Vertical => m.rect.y -= d.delta,
                    Axis::Horizontal => m.rect.x -= d.delta,
                }
            }
        }
        m
    }

    /// The edge with its shared span taken from the preview positions.
    fn preview_edge(&self, e: &Edge) -> Edge {
        let (a, b) = (self.preview(e.a).rect, self.preview(e.b).rect);
        let (s0, s1) = match e.axis {
            Axis::Vertical => (a.y.max(b.y), a.bottom().min(b.bottom())),
            Axis::Horizontal => (a.x.max(b.x), a.right().min(b.right())),
        };
        Edge { s0, s1: s1.max(s0), ..*e }
    }

    fn send(&mut self, mon: usize, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let (px, py) = match self.pending {
            Some((m, x, y)) if m == mon => (x, y),
            _ => (0, 0),
        };
        self.pending = Some((mon, px + dx, py + dy));
        if let Some((tx, ctx)) = &self.events {
            let _ = tx.send(OverlayEvent::Shift { mon, dx, dy });
            ctx.request_repaint();
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

unsafe fn run(
    shared: Arc<Mutex<Option<OverlayData>>>,
    tid: mpsc::Sender<u32>,
    events: mpsc::Sender<OverlayEvent>,
    ctx: egui::Context,
) {
    unsafe {
        let mut msg: MSG = zeroed();
        // Make sure the thread has a message queue before anyone posts to it.
        PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
        STATE.with(|s| s.borrow_mut().events = Some((events, ctx)));
        let _ = tid.send(GetCurrentThreadId());

        let hinst = GetModuleHandleW(null());
        let class = wide("MonitorAlignerRuler");
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinst;
        wc.lpszClassName = class.as_ptr();
        RegisterClassExW(&wc);

        // An invisible top-level window, so display change broadcasts arrive even with the rulers off.
        let title = wide("Display watcher");
        CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null());

        SetTimer(null_mut(), 0, 30, None);

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

/// Puts ruler windows where the data says, reusing existing windows to avoid flicker.
unsafe fn rebuild(data: OverlayData, hinst: *mut core::ffi::c_void, class: &[u16]) {
    let mut strips = layout_strips(&data);
    let old: Vec<Strip> = STATE.with(|s| std::mem::take(&mut s.borrow_mut().strips));

    let mut old = old.into_iter();
    let mut created = Vec::new();
    let title = wide("Ruler");
    for st in &mut strips {
        let r = st.rect;
        match old.next() {
            Some(prev) => {
                st.hwnd = prev.hwnd;
                if prev.rect != r {
                    unsafe { SetWindowPos(st.hwnd, HWND_TOPMOST, r.x, r.y, r.w, r.h, SWP_NOACTIVATE) };
                }
            }
            None => {
                st.hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
                    unsafe { SetLayeredWindowAttributes(st.hwnd, 0, 240, LWA_ALPHA) };
                    created.push(st.hwnd);
                }
            }
        }
    }
    for extra in old {
        unsafe { DestroyWindow(extra.hwnd) };
    }

    let hwnds: Vec<HWND> = strips.iter().map(|s| s.hwnd).collect();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.data = data;
        s.strips = strips;
        s.pending = None;
        if s.drag.as_ref().is_some_and(|d| d.mon >= s.data.edited.len()) {
            s.drag = None;
        }
    });
    for h in created {
        unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
    }
    for h in hwnds {
        unsafe { InvalidateRect(h, null(), 0) };
    }
}

fn layout_strips(d: &OverlayData) -> Vec<Strip> {
    let mut out = Vec::new();
    for (ei, e) in d.edges.iter().enumerate() {
        for (mi, side) in match e.axis {
            Axis::Vertical => [(e.a, Side::Right), (e.b, Side::Left)],
            Axis::Horizontal => [(e.a, Side::Bottom), (e.b, Side::Top)],
        } {
            let m = &d.applied[mi].rect;
            let depth = (104.0 * d.scales.get(mi).copied().unwrap_or(1.0)).round() as i32;
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

fn contains(r: &Rect, p: (i32, i32)) -> bool {
    p.0 >= r.x && p.0 < r.right() && p.1 >= r.y && p.1 < r.bottom()
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
        s.hovered = s
            .strips
            .iter()
            .find(|st| contains(&st.rect, s.cursor))
            .map(|st| st.hwnd)
            .unwrap_or(null_mut());
        s.strips.iter().map(|st| st.hwnd).collect()
    });
    for h in hwnds {
        unsafe { InvalidateRect(h, null(), 0) };
    }
}

fn invalidate_all() {
    let hwnds: Vec<HWND> = STATE.with(|s| s.borrow().strips.iter().map(|st| st.hwnd).collect());
    for h in hwnds {
        unsafe { InvalidateRect(h, null(), 0) };
    }
}

fn cursor_pos() -> (i32, i32) {
    let mut p: POINT = unsafe { zeroed() };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
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
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_DISPLAYCHANGE => {
                // Every ruler gets the broadcast too; one notice is enough.
                let watcher = STATE.with(|s| !s.borrow().strips.iter().any(|st| st.hwnd == h));
                if watcher {
                    STATE.with(|s| {
                        if let Some((tx, ctx)) = &s.borrow().events {
                            let _ = tx.send(OverlayEvent::DisplayChanged);
                            ctx.request_repaint();
                        }
                    });
                }
                DefWindowProcW(h, msg, wp, lp)
            }
            WM_SETCURSOR => {
                let vertical = STATE.with(|s| {
                    let s = s.borrow();
                    s.strips
                        .iter()
                        .find(|st| st.hwnd == h)
                        .map(|st| matches!(st.side, Side::Left | Side::Right))
                        .unwrap_or(true)
                });
                SetCursor(LoadCursorW(null_mut(), if vertical { IDC_SIZENS } else { IDC_SIZEWE }));
                1
            }
            WM_LBUTTONDOWN => {
                let started = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let Some(st) = s.strips.iter().find(|st| st.hwnd == h) else { return false };
                    let (mon, axis) = (st.mon, s.data.edges[st.edge].axis);
                    // Freeze every edge's zero where it is now.
                    let t0: Vec<f64> = s
                        .data
                        .edges
                        .iter()
                        .map(|e| anchor_t(&s.preview_edge(e), s.data.anchor, Some(s.cursor)))
                        .collect();
                    s.drag = Some(Drag { mon, axis, start: cursor_pos(), delta: 0, t0 });
                    true
                });
                if started {
                    SetCapture(h);
                }
                0
            }
            WM_MOUSEMOVE => {
                let moved = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let Some(d) = s.drag.as_mut() else { return false };
                    let p = cursor_pos();
                    let delta = match d.axis {
                        Axis::Vertical => p.1 - d.start.1,
                        Axis::Horizontal => p.0 - d.start.0,
                    };
                    let changed = delta != d.delta;
                    d.delta = delta;
                    changed
                });
                if moved {
                    invalidate_all();
                }
                0
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                let was_dragging = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let Some(d) = s.drag.take() else { return false };
                    // Ruler moved down/right by `delta` means the monitor's desktop start moves up/left.
                    match d.axis {
                        Axis::Vertical => s.send(d.mon, 0, -d.delta),
                        Axis::Horizontal => s.send(d.mon, -d.delta, 0),
                    }
                    true
                });
                if was_dragging && msg == WM_LBUTTONUP {
                    ReleaseCapture();
                }
                invalidate_all();
                0
            }
            WM_MOUSEWHEEL => {
                let wheel = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
                let step = if wp & MK_SHIFT != 0 { 10 } else { 1 };
                let notches = if wheel.abs() >= 120 { wheel / 120 } else { wheel.signum() };
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    if s.drag.is_some() {
                        return;
                    }
                    let Some(st) = s.strips.iter().find(|st| st.hwnd == h) else { return };
                    let (mon, axis) = (st.mon, s.data.edges[st.edge].axis);
                    // Wheel up moves the ruler up, i.e. the monitor's desktop start down.
                    let d = notches * step;
                    match axis {
                        Axis::Vertical => s.send(mon, 0, d),
                        Axis::Horizontal => s.send(mon, d, 0),
                    }
                });
                invalidate_all();
                0
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

const BG: u32 = rgb(16, 18, 24);
const BG_HOVER: u32 = rgb(24, 28, 38);
const EDGE: u32 = rgb(91, 141, 255);
const TICK: u32 = rgb(232, 235, 242);
const TICK_MINOR: u32 = rgb(120, 128, 146);
const ZERO: u32 = rgb(255, 92, 92);
const CURSOR: u32 = rgb(255, 204, 64);
const LABEL_BG: u32 = rgb(34, 39, 52);
const MUTED: u32 = rgb(139, 147, 167);

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
    let Some(e) = d.edges.get(st.edge) else { return };
    if st.mon >= d.edited.len() {
        return;
    }
    let applied = d.applied[st.mon];
    let m = s.preview(st.mon);
    let scale = d.scales.get(st.mon).copied().unwrap_or(1.0);
    let px = |v: f32| (v * scale).round() as i32;

    let vertical = e.axis == Axis::Vertical;
    let ppmm = e.ppmm(&m);
    // Window pixels map to the applied desktop; ticks are placed in the edited desktop.
    let shift = e.start(&m) - e.start(&applied);
    let origin = if vertical { r.y } else { r.x } as f64 + shift;
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
    let dragging_me = s.drag.as_ref().is_some_and(|dr| dr.mon == st.mon);
    let hovered = s.hovered == st.hwnd || dragging_me;

    unsafe {
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        fill(mem, if hovered { BG_HOVER } else { BG }, RECT { left: 0, top: 0, right: w, bottom: h });

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
                fill(mem, LABEL_BG, RECT { left: x - 4, top: y, right: x + tw + 4, bottom: y + th });
            }
            SetTextColor(mem, color);
            TextOutW(mem, x, y, t.as_ptr(), t.len() as i32);
        };

        let t0 = match &s.drag {
            Some(dr) => dr.t0.get(st.edge).copied().unwrap_or(0.0),
            None => anchor_t(&s.preview_edge(e), d.anchor, Some(s.cursor)),
        };
        let lo = ((origin - t0) / ppmm).floor() as i64 - 1;
        let hi = ((origin + len as f64 - t0) / ppmm).ceil() as i64 + 1;
        let fine = ppmm >= 2.5;
        let label_dist = px(34.0);
        for l in lo..=hi {
            let a = (t0 + l as f64 * ppmm - origin).round() as i32;
            if a < 0 || a >= len || l == 0 {
                continue;
            }
            let (tick_len, color) = if l % 10 == 0 {
                (px(28.0), TICK)
            } else if l % 5 == 0 {
                (px(18.0), TICK)
            } else if fine {
                (px(9.0), TICK_MINOR)
            } else {
                continue;
            };
            line(mem, color, 1, to_xy(a, 0), to_xy(a, tick_len));
            // Keep the start of vertical rulers free for the neighbour's number.
            if l % 10 == 0 && !(vertical && a < px(26.0)) {
                label(&l.to_string(), a, label_dist, TICK, false);
            }
        }

        // The zero: on both sides of the edge it marks the same physical point when aligned.
        let a0 = (t0 - origin).round() as i32;
        if (0..len).contains(&a0) {
            line(mem, ZERO, px(2.0).max(1), to_xy(a0, 0), to_xy(a0, depth));
            SelectObject(mem, bold);
            label("0", a0, label_dist, ZERO, true);
            SelectObject(mem, small);
        }

        // Where the cursor would cross the edge, in millimetres from the zero on this panel.
        let tc = e.along(s.cursor) as f64;
        let ac = (tc - (if vertical { r.y } else { r.x }) as f64).round() as i32;
        if (0..len).contains(&ac) && s.drag.is_none() {
            line(mem, CURSOR, 1, to_xy(ac, 0), to_xy(ac, depth));
            let mm = (tc + shift - t0) / ppmm;
            let offset = if vertical { px(10.0) } else { 0 };
            label(&format!("{mm:+.1}"), ac + offset, px(58.0), CURSOR, true);
        }

        // The edge itself and the neighbour's number.
        line(mem, EDGE, px(3.0).max(2), to_xy(0, 0), to_xy(len, 0));
        let other = d.labels.get(e.other(st.mon)).cloned().unwrap_or_default();
        SelectObject(mem, bold);
        let head_at = if vertical { px(12.0) } else { px(40.0) };
        label(&format!("→ {other}"), head_at, px(58.0), EDGE, true);

        // While dragging, show how far the monitor has moved.
        if let Some(dr) = s.drag.as_ref().filter(|dr| dr.mon == st.mon) {
            let mm = dr.delta as f64 / ppmm;
            let at = (if vertical { s.cursor.1 - r.y } else { s.cursor.0 - r.x }).clamp(px(30.0), len - px(30.0));
            if vertical {
                label(&format!("{:+} пикс.", dr.delta), at - px(9.0), px(40.0), CURSOR, true);
                label(&format!("{mm:+.1} мм"), at + px(9.0), px(40.0), CURSOR, true);
            } else {
                label(&format!("{:+} пикс. · {mm:+.1} мм", dr.delta), at, px(58.0), CURSOR, true);
            }
        } else if hovered && s.drag.is_none() {
            SelectObject(mem, small);
            let at = if vertical { len - px(14.0) } else { len - px(90.0) };
            let hint = if vertical { "тяните ↕" } else { "тяните ↔ или колесо" };
            label(hint, at, px(40.0), MUTED, true);
        }

        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);

        SelectObject(mem, old_font);
        DeleteObject(small);
        DeleteObject(bold);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
    }
}
