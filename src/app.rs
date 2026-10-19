//! The layout editor window.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Key, Layout, Margin, PointerButton, Pos2,
    RichText, Sense, Shape, Stroke, StrokeKind, Vec2,
};

use crate::config::Config;
use crate::display::{self, Display};
use crate::layout::{
    Anchor, Axis, Edge, Mon, Rect, anchor_t, attach, find_edges, implied_offset_mm, mismatch_mm,
    target_start,
};
use crate::overlay::{Overlay, OverlayData, OverlayEvent};
use crate::theme::{self, icon};

/// Assumed pixel pitch when the monitor reports no size at all.
const GUESS_MM_PER_PX: f32 = 0.27;
/// Pause after the last change before it is applied in live mode.
const LIVE_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SizeSource {
    Edid,
    User,
    Guess,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Px(i32),
    Mm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusKind {
    Info,
    Busy,
    Ok,
    Warn,
    Error,
}

struct Drag {
    index: usize,
    start_mouse: Pos2,
    start_pos: (i32, i32),
}

/// An apply running on a worker thread.
struct ApplyJob {
    rx: mpsc::Receiver<Result<(), String>>,
    /// Edited positions at the moment of sending, before moving the primary to (0, 0).
    sent: Vec<(i32, i32)>,
    /// How far the whole desktop moves so that the primary ends up at (0, 0).
    shift: (i32, i32),
}

pub struct App {
    ctx: egui::Context,
    config: Config,
    /// Layout as it currently is in Windows.
    displays: Vec<Display>,
    /// Edited positions, same order as `displays`.
    pos: Vec<(i32, i32)>,
    /// Panel size in native orientation and where it came from.
    sizes: Vec<((f32, f32), SizeSource)>,
    /// Positions at program start, by device name, for "undo everything".
    initial: Vec<(String, i32, i32)>,
    selected: Option<usize>,
    step: Step,
    live: bool,
    rulers: bool,
    topmost: bool,
    anchor: Anchor,
    changed_at: Option<Instant>,
    measured: HashMap<(String, String), f32>,
    status: (StatusKind, String),
    overlay: Overlay,
    overlay_events: mpsc::Receiver<OverlayEvent>,
    applying: Option<ApplyJob>,
    // Canvas view: screen position of desktop (0, 0) and screen points per desktop pixel.
    view_offset: Vec2,
    view_scale: f32,
    fit_view: bool,
    last_canvas: egui::Rect,
    /// Floating panels over the canvas last frame; clicks there don't reach the canvas.
    floating: Vec<egui::Rect>,
    anchor_bar: Vec2,
    zoom_bar: Vec2,
    drag: Option<Drag>,
    /// Until when to keep checking the window after the desktop layout changed.
    settle_until: Option<Instant>,
    /// Step of the forced resize that makes the renderer catch up with the real window size.
    resync: Option<(Vec2, bool)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::setup(&cc.egui_ctx);
        let config = Config::load();
        let (overlay, overlay_events) = Overlay::start(cc.egui_ctx.clone());
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            live: config.live,
            anchor: config.anchor,
            config,
            displays: Vec::new(),
            pos: Vec::new(),
            sizes: Vec::new(),
            initial: Vec::new(),
            selected: None,
            step: Step::Px(1),
            rulers: true,
            topmost: false,
            changed_at: None,
            measured: HashMap::new(),
            status: (StatusKind::Info, String::new()),
            overlay,
            overlay_events,
            applying: None,
            view_offset: Vec2::ZERO,
            view_scale: 0.1,
            fit_view: true,
            last_canvas: egui::Rect::NOTHING,
            floating: Vec::new(),
            anchor_bar: Vec2::new(400.0, 46.0),
            zoom_bar: Vec2::new(170.0, 46.0),
            drag: None,
            settle_until: None,
            resync: None,
        };
        app.reload();
        app.initial = app.displays.iter().map(|d| (d.device_name.clone(), d.x, d.y)).collect();
        app.selected = app.displays.iter().position(|d| !d.primary).or(Some(0));
        app.status = (StatusKind::Info, format!("Найдено мониторов: {}", app.displays.len()));
        app
    }

    /// Re-reads the layout from Windows, dropping unapplied edits but keeping the selection.
    fn reload(&mut self) {
        let selected_name = self.selected.and_then(|i| self.displays.get(i)).map(|d| d.device_name.clone());
        self.displays = display::enumerate();
        self.pos = self.displays.iter().map(|d| (d.x, d.y)).collect();
        self.sizes = self.displays.iter().map(|d| self.size_for(d)).collect();
        self.selected = selected_name
            .and_then(|n| self.displays.iter().position(|d| d.device_name == n))
            .or(if self.displays.is_empty() { None } else { Some(0) });
        self.changed_at = None;
        self.push_overlay();
    }

    fn size_for(&self, d: &Display) -> ((f32, f32), SizeSource) {
        if let Some([w, h]) = self.config.sizes.get(&d.key()) {
            return ((*w, *h), SizeSource::User);
        }
        if let Some(mm) = d.edid.as_ref().and_then(|e| e.size_mm) {
            return (mm, SizeSource::Edid);
        }
        let (w, h) = if d.rotated { (d.height, d.width) } else { (d.width, d.height) };
        ((w as f32 * GUESS_MM_PER_PX, h as f32 * GUESS_MM_PER_PX), SizeSource::Guess)
    }

    /// Visible panel size in millimetres in the current orientation.
    fn mm(&self, i: usize) -> (f32, f32) {
        let ((w, h), _) = self.sizes[i];
        if self.displays[i].rotated { (h, w) } else { (w, h) }
    }

    fn mon(&self, i: usize, p: (i32, i32)) -> Mon {
        let d = &self.displays[i];
        let (mm_w, mm_h) = self.mm(i);
        Mon { rect: Rect { x: p.0, y: p.1, w: d.width, h: d.height }, mm_w, mm_h }
    }

    fn edited_mons(&self) -> Vec<Mon> {
        (0..self.displays.len()).map(|i| self.mon(i, self.pos[i])).collect()
    }

    fn applied_mons(&self) -> Vec<Mon> {
        (0..self.displays.len()).map(|i| self.mon(i, (self.displays[i].x, self.displays[i].y))).collect()
    }

    fn has_changes(&self) -> bool {
        self.displays.iter().zip(&self.pos).any(|(d, p)| (d.x, d.y) != *p)
    }

    fn push_overlay(&self) {
        if !self.rulers {
            self.overlay.set(OverlayData::default());
            return;
        }
        let applied = self.applied_mons();
        self.overlay.set(OverlayData {
            edges: find_edges(&applied),
            applied,
            edited: self.edited_mons(),
            labels: self.displays.iter().map(|d| d.number.to_string()).collect(),
            scales: self.displays.iter().map(|d| d.dpi_scale).collect(),
            anchor: self.anchor,
        });
    }

    fn mark_changed(&mut self) {
        self.changed_at = Some(Instant::now());
        self.push_overlay();
    }

    /// Pulls a monitor that floats free or overlaps back to the nearest valid spot.
    fn attach(&mut self, i: usize) {
        let others: Vec<Rect> = (0..self.displays.len()).filter(|&j| j != i).map(|j| self.mon(j, self.pos[j]).rect).collect();
        let p = attach(self.mon(i, self.pos[i]).rect, &others);
        if p != self.pos[i] {
            self.pos[i] = p;
            self.mark_changed();
        }
    }

    fn apply(&mut self) {
        if self.applying.is_some() || self.displays.is_empty() {
            return;
        }
        self.changed_at = None;
        for i in 0..self.displays.len() {
            self.attach(i);
        }
        self.changed_at = None;
        // Windows wants the primary display at (0, 0); everything else is relative to it.
        let primary = self.displays.iter().position(|d| d.primary).unwrap_or(0);
        let shift = self.pos[primary];
        let wanted: Vec<(String, i32, i32)> = self
            .displays
            .iter()
            .zip(&self.pos)
            .map(|(d, &(x, y))| (d.device_name.clone(), x - shift.0, y - shift.1))
            .collect();
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(display::apply_positions(&wanted));
            ctx.request_repaint();
        });
        self.applying = Some(ApplyJob { rx, sent: self.pos.clone(), shift });
        self.status = (StatusKind::Busy, "Применяю…".into());
    }

    fn poll_apply(&mut self) {
        let Some(job) = &self.applying else { return };
        let Ok(result) = job.rx.try_recv() else { return };
        let job = self.applying.take().unwrap();
        self.settle_until = Some(Instant::now() + Duration::from_millis(1500));
        let (sx, sy) = job.shift;
        let edits = self.pos.clone();
        let fresh = display::enumerate();
        if fresh.len() != edits.len() || fresh.iter().zip(&self.displays).any(|(a, b)| a.device_name != b.device_name) {
            self.reload();
            self.fit_view = true;
        } else {
            self.displays = fresh;
            // Keep edits made while the apply was running; everything else follows Windows.
            let mut pending = false;
            for (i, d) in self.displays.iter().enumerate() {
                if edits[i] == job.sent[i] {
                    self.pos[i] = (d.x, d.y);
                } else {
                    self.pos[i] = (edits[i].0 - sx, edits[i].1 - sy);
                    pending = true;
                }
            }
            // Keep the picture still on the canvas if the origin moved.
            self.view_offset += Vec2::new(sx as f32, sy as f32) * self.view_scale;
            if let Some(d) = &mut self.drag {
                d.start_pos = (d.start_pos.0 - sx, d.start_pos.1 - sy);
            }
            if pending {
                self.mark_changed();
            } else {
                self.push_overlay();
            }
        }
        let adjusted = self
            .displays
            .iter()
            .zip(&job.sent)
            .any(|(d, &(x, y))| (d.x, d.y) != (x - sx, y - sy));
        self.status = match result {
            Err(e) => (StatusKind::Error, format!("Ошибка: {e}")),
            Ok(()) if adjusted => (StatusKind::Warn, "Windows поправил положение мониторов".into()),
            Ok(()) => (StatusKind::Ok, "Применено".into()),
        };
    }

    fn revert_to_initial(&mut self) {
        for (i, d) in self.displays.iter().enumerate() {
            if let Some((_, x, y)) = self.initial.iter().find(|(n, _, _)| *n == d.device_name) {
                self.pos[i] = (*x, *y);
            }
        }
        self.push_overlay();
        self.apply();
    }

    fn step_px(&self, i: usize, axis_x: bool) -> i32 {
        match self.step {
            Step::Px(n) => n,
            Step::Mm => {
                let m = self.mon(i, self.pos[i]);
                let p = if axis_x { m.ppmm_x() } else { m.ppmm_y() };
                (p.round() as i32).max(1)
            }
        }
    }

    fn nudge(&mut self, i: usize, dx: i32, dy: i32) {
        let (sx, sy) = (self.step_px(i, true), self.step_px(i, false));
        self.pos[i].0 += dx * sx;
        self.pos[i].1 += dy * sy;
        self.mark_changed();
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.overlay_events.try_recv() {
            match ev {
                OverlayEvent::Shift { mon, dx, dy } if mon < self.pos.len() => {
                    self.pos[mon].0 += dx;
                    self.pos[mon].1 += dy;
                    self.selected = Some(mon);
                    self.mark_changed();
                }
                _ => {}
            }
        }

        let Some(i) = self.selected else { return };
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (mut dx, mut dy) = (0, 0);
        let mult = ctx.input(|inp| {
            if inp.key_pressed(Key::ArrowLeft) {
                dx -= 1;
            }
            if inp.key_pressed(Key::ArrowRight) {
                dx += 1;
            }
            if inp.key_pressed(Key::ArrowUp) {
                dy -= 1;
            }
            if inp.key_pressed(Key::ArrowDown) {
                dy += 1;
            }
            if inp.modifiers.shift { 10 } else { 1 }
        });
        if dx != 0 || dy != 0 {
            self.nudge(i, dx * mult, dy * mult);
        }
        if ctx.input(|inp| inp.key_pressed(Key::Enter)) && self.has_changes() {
            self.apply();
        }
    }

    fn live_tick(&mut self, ctx: &egui::Context) {
        let Some(t) = self.changed_at else { return };
        if !self.live {
            return;
        }
        if self.drag.is_none() && self.applying.is_none() && t.elapsed() >= LIVE_DELAY {
            if self.has_changes() {
                self.apply();
            } else {
                self.changed_at = None;
            }
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn save_config(&mut self) {
        self.config.anchor = self.anchor;
        self.config.live = self.live;
        self.config.save();
    }

    // ---------------------------------------------------------------- header

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(theme::RADIUS), theme::ACCENT);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, icon::RULER, FontId::proportional(16.0), Color32::WHITE);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(theme::strong("Выравнивание мониторов").size(15.0));
                let (x0, y0, x1, y1) = self.bounds(&self.applied_mons());
                ui.label(
                    RichText::new(format!("{} {} · рабочий стол {}×{}", self.displays.len(), monitors_word(self.displays.len()), x1 - x0, y1 - y0))
                        .size(12.0)
                        .color(theme::MUTED),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let changes = self.has_changes();
                if theme::primary_button(ui, &format!("{}  Применить", icon::CHECK), changes && self.applying.is_none())
                    .on_hover_text("Записать положение мониторов в Windows (Enter)")
                    .clicked()
                {
                    self.apply();
                }
                if theme::ghost_button(ui, &format!("{}  Как было", icon::HISTORY))
                    .on_hover_text("Вернуть раскладку, которая была при запуске программы")
                    .clicked()
                {
                    self.revert_to_initial();
                }
                if changes
                    && theme::ghost_button(ui, &format!("{}  Сбросить", icon::UNDO))
                        .on_hover_text("Забыть неприменённые правки")
                        .clicked()
                {
                    self.pos = self.displays.iter().map(|d| (d.x, d.y)).collect();
                    self.changed_at = None;
                    self.push_overlay();
                }
                if theme::ghost_button(ui, icon::REFRESH).on_hover_text("Перечитать мониторы из Windows").clicked() {
                    self.reload();
                    self.fit_view = true;
                    self.status = (StatusKind::Info, format!("Найдено мониторов: {}", self.displays.len()));
                }
                ui.add_space(8.0);
                let (kind, text) = if self.applying.is_some() {
                    (StatusKind::Busy, "Применяю…".to_string())
                } else if changes {
                    (StatusKind::Warn, if self.live { "Сейчас применится".into() } else { "Есть неприменённые правки".into() })
                } else {
                    self.status.clone()
                };
                let color = match kind {
                    StatusKind::Info => theme::MUTED,
                    StatusKind::Busy => theme::ACCENT,
                    StatusKind::Ok => theme::GREEN,
                    StatusKind::Warn => theme::AMBER,
                    StatusKind::Error => theme::RED,
                };
                if kind != StatusKind::Info {
                    theme::status(ui, color, &text);
                }
            });
        });
    }

    // ---------------------------------------------------------------- side panel

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        theme::caption(ui, "Мониторы");
        ui.spacing_mut().item_spacing.y = 8.0;
        for i in 0..self.displays.len() {
            self.monitor_row(ui, i);
        }
        ui.add_space(8.0);

        let Some(i) = self.selected.filter(|&i| i < self.displays.len()) else {
            theme::card(ui, "Монитор", |ui| ui.label("Выберите монитор на схеме"));
            return;
        };
        self.position_card(ui, i);
        self.screen_card(ui, i);
        self.edge_cards(ui, i);
    }

    fn monitor_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let d = &self.displays[i];
        let sel = self.selected == Some(i);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
        let p = ui.painter();
        let r = CornerRadius::same(theme::RADIUS);
        let fill = if sel {
            theme::ACCENT_DIM
        } else if resp.hovered() {
            theme::CARD_HI
        } else {
            theme::CARD
        };
        p.rect(rect, r, fill, Stroke::new(1.0, if sel { theme::ACCENT } else { theme::STROKE }), StrokeKind::Inside);
        let badge = egui::Rect::from_center_size(Pos2::new(rect.left() + 26.0, rect.center().y), Vec2::splat(28.0));
        p.rect_filled(badge, CornerRadius::same(theme::RADIUS - 2), if sel { theme::ACCENT } else { theme::CARD_HI });
        let strong = theme::strong_family();
        p.text(badge.center(), Align2::CENTER_CENTER, d.number.to_string(), FontId::new(14.0, strong.clone()), Color32::WHITE);
        p.text(Pos2::new(rect.left() + 50.0, rect.top() + 17.0), Align2::LEFT_CENTER, &d.name, FontId::new(14.0, strong), theme::TEXT);
        p.text(
            Pos2::new(rect.left() + 50.0, rect.top() + 35.0),
            Align2::LEFT_CENTER,
            format!("{}×{} · {} Гц · {:.0}%", d.width, d.height, d.refresh, d.dpi_scale * 100.0),
            FontId::proportional(12.0),
            theme::MUTED,
        );
        if d.primary {
            p.text(Pos2::new(rect.right() - 12.0, rect.center().y), Align2::RIGHT_CENTER, "основной", FontId::proportional(12.0), theme::MUTED);
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.selected = Some(i);
        }
    }

    fn position_card(&mut self, ui: &mut egui::Ui, i: usize) {
        let title = format!("Положение монитора {}", self.displays[i].number);
        theme::card(ui, &title, |ui| {
            ui.horizontal(|ui| {
                let (mut x, mut y) = self.pos[i];
                let mut changed = false;
                for (label, v) in [("X", &mut x), ("Y", &mut y)] {
                    ui.label(RichText::new(label).color(theme::MUTED));
                    changed |= ui.add_sized([96.0, 30.0], egui::DragValue::new(v).speed(1).suffix(" пикс.")).changed();
                }
                if changed {
                    self.pos[i] = (x, y);
                    self.mark_changed();
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // Arrow pad.
                let b = Vec2::splat(34.0);
                egui::Grid::new("pad").spacing([4.0, 4.0]).show(ui, |ui| {
                    ui.label("");
                    if pad_button(ui, icon::UP, b).clicked() {
                        self.nudge(i, 0, -1);
                    }
                    ui.label("");
                    ui.end_row();
                    if pad_button(ui, icon::LEFT, b).clicked() {
                        self.nudge(i, -1, 0);
                    }
                    if pad_button(ui, icon::DOWN, b).clicked() {
                        self.nudge(i, 0, 1);
                    }
                    if pad_button(ui, icon::RIGHT, b).clicked() {
                        self.nudge(i, 1, 0);
                    }
                    ui.end_row();
                });
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("Шаг").size(12.0).color(theme::MUTED));
                    theme::segmented(ui, &mut self.step, &[(Step::Px(1), "1"), (Step::Px(5), "5"), (Step::Px(10), "10"), (Step::Mm, "1 мм")]);
                });
            });
            ui.label(RichText::new("Стрелки на клавиатуре, Shift — в 10 раз больше. Линейки на экранах можно тянуть мышью и крутить колесом.").size(12.0).color(theme::MUTED));
        });
        ui.add_space(4.0);
    }

    fn screen_card(&mut self, ui: &mut egui::Ui, i: usize) {
        let d = self.displays[i].clone();
        theme::card(ui, "Экран", |ui| {
            let (mw, mh) = self.mm(i);
            let diag = (mw * mw + mh * mh).sqrt() / 25.4;
            let ppi = d.width as f32 / (mw / 25.4);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 28.0;
                theme::stat(ui, "Диагональ", &format!("{diag:.1}″"));
                theme::stat(ui, "Плотность", &format!("{ppi:.0} PPI"));
                theme::stat(ui, "Масштаб", &format!("{:.0}%", d.dpi_scale * 100.0));
            });

            ui.add_space(4.0);
            let ((mut w, mut h), source) = self.sizes[i];
            ui.horizontal(|ui| {
                ui.label(RichText::new("Видимая область").color(theme::MUTED));
                let cw = ui.add_sized([70.0, 28.0], egui::DragValue::new(&mut w).speed(0.1).range(50.0..=3000.0).max_decimals(1)).changed();
                ui.label("×");
                let ch = ui.add_sized([70.0, 28.0], egui::DragValue::new(&mut h).speed(0.1).range(50.0..=3000.0).max_decimals(1)).changed();
                ui.label(RichText::new("мм").color(theme::MUTED));
                if cw || ch {
                    self.sizes[i] = ((w, h), SizeSource::User);
                    self.config.sizes.insert(d.key(), [w, h]);
                    self.save_config();
                    self.push_overlay();
                }
            });
            ui.horizontal(|ui| {
                let (color, text) = match source {
                    SizeSource::Edid => (theme::MUTED, "из EDID, может быть округлено"),
                    SizeSource::User => (theme::GREEN, "измерено вручную"),
                    SizeSource::Guess => (theme::AMBER, "монитор не сообщил размер, измерьте его"),
                };
                ui.label(RichText::new(text).size(12.0).color(color));
                if source == SizeSource::User && ui.small_button("из EDID").clicked() {
                    self.config.sizes.remove(&d.key());
                    self.save_config();
                    self.sizes[i] = self.size_for(&d);
                    self.push_overlay();
                }
            });
        });
        ui.add_space(4.0);
    }

    fn edge_cards(&mut self, ui: &mut egui::Ui, i: usize) {
        let mons = self.edited_mons();
        let edges: Vec<Edge> = find_edges(&mons).into_iter().filter(|e| e.a == i || e.b == i).collect();
        if edges.is_empty() {
            theme::card(ui, "Стыки", |ui| {
                ui.label(RichText::new("Монитор не касается других. При применении он будет придвинут к ближайшему.").color(theme::AMBER));
            });
            return;
        }
        for e in edges {
            let j = e.other(i);
            let me_b = e.b == i;
            let (side, start_word, low, high, presets) = match (e.axis, me_b) {
                (Axis::Vertical, true) => ("слева", "Верх", "ниже", "выше", ["Верх", "Центр", "Низ"]),
                (Axis::Vertical, false) => ("справа", "Верх", "ниже", "выше", ["Верх", "Центр", "Низ"]),
                (Axis::Horizontal, true) => ("сверху", "Левый край", "правее", "левее", ["Лево", "Центр", "Право"]),
                (Axis::Horizontal, false) => ("снизу", "Левый край", "правее", "левее", ["Лево", "Центр", "Право"]),
            };
            let other = self.displays[j].clone();
            let anchor = self.anchor;
            let title = format!("Стык с монитором {} · {side}", other.number);
            theme::card(ui, &title, |ui| {

                let t0 = anchor_t(&e, anchor, None);
                // Offset of this monitor's side relative to the neighbour's, positive = lower / further right.
                let d_ab = implied_offset_mm(&e, &mons, t0);
                let mine = if me_b { d_ab } else { -d_ab };
                let px_off = if me_b {
                    e.start(&mons[e.b]) - e.start(&mons[e.a])
                } else {
                    e.start(&mons[e.a]) - e.start(&mons[e.b])
                };
                ui.horizontal(|ui| {
                    ui.label(theme::strong(format!("{:.1} мм", mine.abs())).size(22.0));
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(format!("{start_word} {} края соседа", if mine >= 0.0 { low } else { high })).size(12.0).color(theme::MUTED));
                        ui.label(RichText::new(format!("{:+} пикс.", px_off as i32)).size(12.0).color(theme::MUTED));
                    });
                });
                let err = mismatch_mm(&e, &mons, t0, e.s0 as f64).abs().max(mismatch_mm(&e, &mons, t0, e.s1 as f64).abs());
                if err >= 0.05 {
                    ui.label(
                        RichText::new(format!("Плотность пикселей разная: без скачка курсор проходит в точке «{}», к концам стыка расхождение до {err:.1} мм.", anchor_name(anchor).to_lowercase()))
                            .size(12.0)
                            .color(theme::AMBER),
                    );
                }

                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Выровнять края").color(theme::MUTED));
                    let (la, lb) = (e.len_mm(&mons[e.a]), e.len_mm(&mons[e.b]));
                    for (name, d) in presets.iter().zip([0.0, (la - lb) / 2.0, la - lb]) {
                        if ui.button(*name).on_hover_text("Если сами экраны стоят ровно по этому краю").clicked() {
                            self.move_to(i, &e, &mons, d, anchor, me_b);
                        }
                    }
                });

                let key = (self.displays[i].device_name.clone(), other.device_name.clone());
                let mut measured = *self.measured.get(&key).unwrap_or(&0.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Измерено: {low} на")).color(theme::MUTED));
                    ui.add_sized([80.0, 28.0], egui::DragValue::new(&mut measured).speed(0.1).max_decimals(1).suffix(" мм"));
                    if ui
                        .button("Подогнать")
                        .on_hover_text(format!(
                            "Приложите линейку поперёк стыка и измерьте, насколько этот экран {low} соседа. Если {high}, введите минус."
                        ))
                        .clicked()
                    {
                        let d = if me_b { measured as f64 } else { -measured as f64 };
                        self.move_to(i, &e, &mons, d, anchor, me_b);
                    }
                });
                self.measured.insert(key, measured);
            });
            ui.add_space(4.0);
        }
    }

    fn move_to(&mut self, i: usize, e: &Edge, mons: &[Mon], d: f64, anchor: Anchor, move_b: bool) {
        let v = target_start(e, mons, d, anchor, move_b);
        match e.axis {
            Axis::Vertical => self.pos[i].1 = v,
            Axis::Horizontal => self.pos[i].0 = v,
        }
        self.mark_changed();
    }

    // ---------------------------------------------------------------- canvas

    /// Controls floating over the canvas: switches, ruler zero, zoom and a hint.
    fn canvas_overlays(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        let inner = area.shrink(12.0);
        let mut floating = Vec::new();

        let mut tl = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Min)));
        theme::floating(&mut tl, |ui| self.toggles(ui));
        let first = tl.min_rect();
        floating.push(first);

        // egui lays out in one pass, so right-aligned panels use their size from the previous frame.
        let at_right = |size: Vec2, top: f32| {
            egui::Rect::from_min_max(Pos2::new((inner.right() - size.x).max(inner.left()), top), inner.max)
        };

        // Ruler zero goes to the right corner when there is room, otherwise under the switches.
        let wide = inner.width() >= first.width() + self.anchor_bar.x + 16.0;
        let rect = if wide {
            at_right(self.anchor_bar, inner.top())
        } else {
            egui::Rect::from_min_max(Pos2::new(inner.left(), first.bottom() + 8.0), inner.max)
        };
        let mut tr = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Min)));
        theme::floating(&mut tr, |ui| self.anchor_picker(ui));
        floating.push(tr.min_rect());
        self.remember_size(&mut 0, tr.min_rect().size(), ui.ctx());

        let zoom_rect = egui::Rect::from_min_max(inner.max - self.zoom_bar, inner.max);
        let mut br = ui.new_child(egui::UiBuilder::new().max_rect(zoom_rect).layout(Layout::left_to_right(Align::Min)));
        theme::floating(&mut br, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let c = area.center().to_vec2();
            let zoom = |f: f32, this: &mut Self| {
                this.view_offset = c + (this.view_offset - c) * f;
                this.view_scale *= f;
            };
            if theme::icon_button(ui, icon::ZOOM_OUT).on_hover_text("Отдалить").clicked() {
                zoom(1.0 / 1.25, self);
            }
            ui.add_sized([46.0, 30.0], egui::Label::new(RichText::new(format!("{:.0}%", self.view_scale * 100.0)).size(12.0).color(theme::MUTED)));
            if theme::icon_button(ui, icon::ZOOM_IN).on_hover_text("Приблизить").clicked() {
                zoom(1.25, self);
            }
            if theme::icon_button(ui, icon::FIT).on_hover_text("Показать всю раскладку").clicked() {
                self.fit_view = true;
            }
        });
        floating.push(br.min_rect());
        self.remember_size(&mut 1, br.min_rect().size(), ui.ctx());

        // The hint only where it fits next to the zoom panel.
        let hint_rect = egui::Rect::from_min_max(inner.min, Pos2::new(inner.right() - self.zoom_bar.x - 16.0, inner.bottom()));
        let mut bl = ui.new_child(egui::UiBuilder::new().max_rect(hint_rect).layout(Layout::bottom_up(Align::Min)));
        if hint_rect.width() >= 600.0 {
            bl.label(
            RichText::new("Тяните мониторы мышью · Alt — без прилипания · колесо — масштаб · правая кнопка — сдвиг вида")
                .size(12.0)
                .color(theme::MUTED.gamma_multiply(0.8)),
            );
        }

        self.floating = floating;
    }

    fn remember_size(&mut self, which: &mut u8, size: Vec2, ctx: &egui::Context) {
        let slot = if *which == 0 { &mut self.anchor_bar } else { &mut self.zoom_bar };
        if (*slot - size).length() > 0.5 {
            *slot = size;
            ctx.request_repaint();
        }
    }

    fn toggles(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 18.0;
        if theme::toggle(ui, &mut self.rulers, "Линейки на экранах").changed() {
            self.push_overlay();
        }
        if theme::toggle(ui, &mut self.live, "Применять сразу").changed() {
            self.save_config();
        }
        if theme::toggle(ui, &mut self.topmost, "Поверх окон").changed() {
            let level = if self.topmost { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
    }

    fn anchor_picker(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new("Ноль линеек").color(theme::MUTED))
            .on_hover_text("Точка стыка, где курсор переходит без скачка. От неё отсчитываются миллиметры на линейках.");
        if theme::segmented(
            ui,
            &mut self.anchor,
            &[(Anchor::Start, "Начало"), (Anchor::Center, "Центр"), (Anchor::End, "Конец"), (Anchor::Cursor, "Курсор")],
        ) {
            self.save_config();
            self.push_overlay();
        }
    }

    fn bounds(&self, mons: &[Mon]) -> (i32, i32, i32, i32) {
        if mons.is_empty() {
            return (0, 0, 1, 1);
        }
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for m in mons {
            x0 = x0.min(m.rect.x);
            y0 = y0.min(m.rect.y);
            x1 = x1.max(m.rect.right());
            y1 = y1.max(m.rect.bottom());
        }
        (x0, y0, x1, y1)
    }

    fn fit(&mut self, area: egui::Rect) {
        let (x0, y0, x1, y1) = self.bounds(&self.edited_mons());
        let margin = 48.0;
        let bw = (x1 - x0).max(1) as f32;
        let bh = (y1 - y0).max(1) as f32;
        self.view_scale = ((area.width() - 2.0 * margin) / bw).min((area.height() - 2.0 * margin) / bh).max(0.001);
        let center = Vec2::new((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
        self.view_offset = area.center().to_vec2() - center * self.view_scale;
    }

    fn to_screen(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(x, y) * self.view_scale + self.view_offset
    }

    fn screen_rect(&self, r: &Rect) -> egui::Rect {
        egui::Rect::from_min_max(self.to_screen(r.x as f32, r.y as f32), self.to_screen(r.right() as f32, r.bottom() as f32))
    }

    fn hit(&self, p: Pos2) -> Option<usize> {
        let mons = self.edited_mons();
        // Selected one first so it can be grabbed even if it overlaps another.
        let order = self.selected.into_iter().chain(0..mons.len());
        order.into_iter().find(|&i| self.screen_rect(&mons[i].rect).contains(p))
    }

    /// Pulls a dragged monitor to nearby edges of the others.
    fn snap(&self, i: usize, x: f64, y: f64) -> (i32, i32) {
        let me = self.mon(i, (0, 0)).rect;
        let thr = 10.0 / self.view_scale as f64;
        let (mut bx, mut by) = ((x, thr), (y, thr));
        for j in 0..self.displays.len() {
            if j == i {
                continue;
            }
            let o = self.mon(j, self.pos[j]).rect;
            for c in [o.right(), o.x - me.w, o.x, o.right() - me.w] {
                let dist = (c as f64 - x).abs();
                if dist < bx.1 {
                    bx = (c as f64, dist);
                }
            }
            for c in [o.bottom(), o.y - me.h, o.y, o.bottom() - me.h] {
                let dist = (c as f64 - y).abs();
                if dist < by.1 {
                    by = (c as f64, dist);
                }
            }
        }
        (bx.0.round() as i32, by.0.round() as i32)
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let area = resp.rect;
        let hover = ui.input(|i| i.pointer.hover_pos());
        let pointer_on_floating = hover.is_some_and(|p| self.floating.iter().any(|r| r.contains(p)));
        if self.fit_view || (area.size() != self.last_canvas.size() && self.drag.is_none()) {
            self.fit(area);
            self.fit_view = false;
        }
        self.last_canvas = area;

        // Zoom around the cursor, pan with the right or middle button.
        if let Some(hp) = resp.hover_pos().filter(|_| !pointer_on_floating) {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let f = (scroll * 0.004).exp();
                self.view_offset = hp.to_vec2() + (self.view_offset - hp.to_vec2()) * f;
                self.view_scale *= f;
            }
        }
        if resp.dragged_by(PointerButton::Secondary) || resp.dragged_by(PointerButton::Middle) {
            self.view_offset += resp.drag_delta();
        }

        if resp.drag_started_by(PointerButton::Primary) && !pointer_on_floating {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = self.hit(p) {
                    self.selected = Some(i);
                    self.drag = Some(Drag { index: i, start_mouse: p, start_pos: self.pos[i] });
                }
            }
        }
        if let Some(d) = &self.drag {
            let i = d.index;
            if resp.dragged_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    let delta = (p - d.start_mouse) / self.view_scale;
                    let x = d.start_pos.0 as f64 + delta.x as f64;
                    let y = d.start_pos.1 as f64 + delta.y as f64;
                    let no_snap = ui.input(|inp| inp.modifiers.alt);
                    let new = if no_snap { (x.round() as i32, y.round() as i32) } else { self.snap(i, x, y) };
                    if new != self.pos[i] {
                        self.pos[i] = new;
                        self.mark_changed();
                    }
                }
            } else {
                self.drag = None;
                self.attach(i);
            }
        }
        if resp.clicked() && !pointer_on_floating {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = self.hit(p) {
                    self.selected = Some(i);
                }
            }
        }

        let painter = painter.with_clip_rect(area.expand(1.0));
        painter.rect_filled(area, CornerRadius::same(theme::RADIUS), theme::BG);
        self.draw_grid(&painter.with_clip_rect(area.shrink(2.0)), area);
        painter.rect_stroke(area, CornerRadius::same(theme::RADIUS), Stroke::new(1.0, theme::STROKE), StrokeKind::Inside);
        let painter = painter.with_clip_rect(area.shrink(1.0));

        let mons = self.edited_mons();
        let applied = self.applied_mons();
        let strong = theme::strong_family();

        // Where monitors are right now in Windows, if they've been moved since.
        for (i, m) in applied.iter().enumerate() {
            if m.rect != mons[i].rect {
                let r = self.screen_rect(&m.rect);
                let s = Stroke::new(1.0, theme::MUTED);
                let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                painter.extend(Shape::dashed_line(&pts, s, 7.0, 5.0));
            }
        }

        for (i, m) in mons.iter().enumerate() {
            let d = &self.displays[i];
            let r = self.screen_rect(&m.rect);
            let sel = self.selected == Some(i);
            let overlaps = mons.iter().enumerate().any(|(j, o)| j != i && o.rect.intersects(&m.rect));

            let fill = if sel { theme::ACCENT_DIM } else { theme::CARD };
            painter.rect_filled(r, CornerRadius::same(4), fill);
            let border = if overlaps {
                theme::RED
            } else if sel {
                theme::ACCENT
            } else {
                Color32::from_rgb(84, 84, 94)
            };
            painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(if sel { 2.0 } else { 1.0 }, border), StrokeKind::Inside);

            let big = (r.height() * 0.32).clamp(12.0, 84.0);
            painter.text(
                r.center() - Vec2::new(0.0, big * 0.3),
                Align2::CENTER_CENTER,
                d.number.to_string(),
                FontId::new(big, strong.clone()),
                if sel { Color32::WHITE } else { theme::MUTED },
            );
            if r.width() > 140.0 && r.height() > 90.0 {
                let (mw, mh) = self.mm(i);
                painter.text(r.center() + Vec2::new(0.0, big * 0.42), Align2::CENTER_TOP, &d.name, FontId::new(13.0, strong.clone()), theme::TEXT);
                painter.text(
                    r.center() + Vec2::new(0.0, big * 0.42 + 19.0),
                    Align2::CENTER_TOP,
                    format!("{}×{} · {:.0}×{:.0} мм", d.width, d.height, mw, mh),
                    FontId::proportional(12.0),
                    theme::TEXT.gamma_multiply(0.55),
                );
                // Desktop position in the corner.
                let g = painter.layout_no_wrap(format!("{}, {}", m.rect.x, m.rect.y), FontId::monospace(11.0), theme::TEXT.gamma_multiply(0.7));
                painter.galley(r.left_top() + Vec2::new(10.0, 8.0), g, theme::MUTED);
            }
        }

        // Shared edges: a glowing seam with its offset.
        for e in find_edges(&mons) {
            let (a, b) = match e.axis {
                Axis::Vertical => (self.to_screen(e.pos as f32, e.s0 as f32), self.to_screen(e.pos as f32, e.s1 as f32)),
                Axis::Horizontal => (self.to_screen(e.s0 as f32, e.pos as f32), self.to_screen(e.s1 as f32, e.pos as f32)),
            };
            painter.line_segment([a, b], Stroke::new(2.0, theme::GREEN));
            let off = e.start(&mons[e.b]) - e.start(&mons[e.a]);
            let mm = implied_offset_mm(&e, &mons, anchor_t(&e, self.anchor, None));
            let mid = a + (b - a) / 2.0;
            let text = format!("{:+} пикс. · {:+.1} мм", off as i32, mm);
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), theme::GREEN);
            let pill = egui::Rect::from_center_size(mid, galley.size() + Vec2::new(16.0, 8.0));
            painter.rect(pill, CornerRadius::same(4), theme::PANEL, Stroke::new(1.0, theme::GREEN), StrokeKind::Inside);
            painter.galley(pill.center() - galley.size() / 2.0, galley, theme::GREEN);
        }

        self.canvas_overlays(ui, area);
    }

    fn draw_grid(&self, painter: &egui::Painter, area: egui::Rect) {
        // Dots every 100, 500 or 1000 desktop pixels, whichever is at least ~24 points apart.
        let step = [100.0, 500.0, 1000.0, 5000.0].into_iter().find(|s| s * self.view_scale >= 24.0).unwrap_or(10000.0);
        let to_desk = |p: Pos2| (p - self.view_offset.to_pos2()) / self.view_scale;
        let (a, b) = (to_desk(area.left_top()), to_desk(area.right_bottom()));
        let color = Color32::from_rgb(52, 52, 58);
        let mut x = (a.x / step).floor() * step;
        while x <= b.x {
            let mut y = (a.y / step).floor() * step;
            while y <= b.y {
                painter.circle_filled(self.to_screen(x, y), 1.2, color);
                y += step;
            }
            x += step;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_apply();
        self.keep_window_in_sync(&ctx, frame);
        self.handle_input(&ctx);
        self.live_tick(&ctx);

        egui::Panel::top("header")
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(16, 10)).stroke(Stroke::new(1.0, theme::STROKE)))
            .show(ui, |ui| self.header(ui));
        egui::Panel::right("side")
            .resizable(false)
            .exact_size(392.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin { left: 14, right: 6, top: 14, bottom: 14 }))
            .show(ui, |ui| {
                // Right margin inside the scroll area keeps cards clear of the scrollbar.
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    egui::Frame::new().inner_margin(Margin { left: 0, right: 10, top: 0, bottom: 0 }).show(ui, |ui| self.side_panel(ui));
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin { left: 14, right: 12, top: 14, bottom: 14 }))
            .show(ui, |ui| self.canvas(ui));
    }
}

impl App {
    /// After Windows rearranges the desktop it can move and resize this window without the
    /// renderer noticing: the picture then sits off-centre, edges flicker and the pointer misses
    /// buttons. Compare the real client size with what egui draws and force a resize if they differ.
    fn keep_window_in_sync(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if let Some((size, grown)) = self.resync.take() {
            if grown {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.request_repaint();
            }
            return;
        }
        let Some(until) = self.settle_until else { return };
        if Instant::now() > until {
            self.settle_until = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(50));

        let Some((cw, ch)) = client_size(frame) else { return };
        let ppp = ctx.pixels_per_point();
        let drawn = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size() * ppp);
        let Some(drawn) = drawn else { return };
        if (drawn.x - cw as f32).abs() > 1.0 || (drawn.y - ch as f32).abs() > 1.0 {
            // Same size would be a no-op for Windows, so grow by a pixel and come back next frame.
            let real = Vec2::new(cw as f32, ch as f32) / ppp;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(real + Vec2::new(1.0, 0.0)));
            self.resync = Some((real, true));
            ctx.request_repaint();
        }
    }
}

fn client_size(frame: &eframe::Frame) -> Option<(i32, i32)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return None };
    let mut r: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };
    let ok = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(h.hwnd.get() as _, &mut r) };
    (ok != 0).then_some((r.right - r.left, r.bottom - r.top))
}

fn pad_button(ui: &mut egui::Ui, glyph: &str, size: Vec2) -> egui::Response {
    ui.add_sized(size, egui::Button::new(RichText::new(glyph).size(13.0)).corner_radius(CornerRadius::same(8)))
}

fn anchor_name(a: Anchor) -> &'static str {
    match a {
        Anchor::Start => "Начало стыка",
        Anchor::Center => "Центр стыка",
        Anchor::End => "Конец стыка",
        Anchor::Cursor => "Под курсором",
    }
}

fn monitors_word(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => "монитор",
        (2..=4, r) if !(12..=14).contains(&r) => "монитора",
        _ => "мониторов",
    }
}
