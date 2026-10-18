//! The layout editor window.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, PointerButton, Pos2, Sense, Stroke,
    StrokeKind, Vec2,
};

use crate::config::Config;
use crate::display::{self, Display};
use crate::layout::{
    Anchor, Axis, Edge, Mon, Rect, anchor_t, find_edges, implied_offset_mm, mismatch_mm,
    target_start,
};
use crate::overlay::{Overlay, OverlayData};

/// Assumed pixel pitch when the monitor reports no size at all.
const GUESS_MM_PER_PX: f32 = 0.27;
const LIVE_DELAY: Duration = Duration::from_millis(350);

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

struct Drag {
    index: usize,
    start_mouse: Pos2,
    start_pos: (i32, i32),
}

pub struct App {
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
    status: String,
    overlay: Overlay,
    // Canvas view: screen position of desktop (0, 0) and screen points per desktop pixel.
    view_offset: Vec2,
    view_scale: f32,
    fit_view: bool,
    last_canvas: egui::Rect,
    drag: Option<Drag>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        let config = Config::load();
        let mut app = Self {
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
            status: String::new(),
            overlay: Overlay::start(),
            view_offset: Vec2::ZERO,
            view_scale: 0.1,
            fit_view: true,
            last_canvas: egui::Rect::NOTHING,
            drag: None,
        };
        app.reload();
        app.initial = app.displays.iter().map(|d| (d.device_name.clone(), d.x, d.y)).collect();
        app.selected = app.displays.iter().position(|d| !d.primary).or(Some(0));
        app.status = format!("Найдено мониторов: {}", app.displays.len());
        app
    }

    /// Re-reads the layout from Windows, keeping the selection.
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
        let mons = self.applied_mons();
        self.overlay.set(OverlayData {
            edges: find_edges(&mons),
            mons,
            labels: self.displays.iter().map(|d| d.number.to_string()).collect(),
            scales: self.displays.iter().map(|d| d.dpi_scale).collect(),
            anchor: self.anchor,
        });
    }

    fn mark_changed(&mut self) {
        self.changed_at = Some(Instant::now());
    }

    fn apply(&mut self) {
        self.changed_at = None;
        // Windows wants the primary display at (0, 0); everything else is relative to it.
        let primary = self.displays.iter().position(|d| d.primary).unwrap_or(0);
        let (px, py) = self.pos[primary];
        let wanted: Vec<(String, i32, i32)> = self
            .displays
            .iter()
            .zip(&self.pos)
            .map(|(d, &(x, y))| (d.device_name.clone(), x - px, y - py))
            .collect();
        let result = display::apply_positions(&wanted);
        self.reload();
        // Keep the picture still on the canvas if the origin moved.
        self.view_offset += Vec2::new(px as f32, py as f32) * self.view_scale;
        self.status = match result {
            Err(e) => format!("Ошибка: {e}"),
            Ok(()) => {
                let adjusted = wanted.iter().any(|(n, x, y)| {
                    self.displays.iter().any(|d| d.device_name == *n && (d.x, d.y) != (*x, *y))
                });
                if adjusted {
                    "Применено, но Windows поправил положение (мониторы должны касаться друг друга и не перекрываться)".into()
                } else {
                    "Применено".into()
                }
            }
        };
    }

    fn revert_to_initial(&mut self) {
        for (i, d) in self.displays.iter().enumerate() {
            if let Some((_, x, y)) = self.initial.iter().find(|(n, _, _)| *n == d.device_name) {
                self.pos[i] = (*x, *y);
            }
        }
        self.apply();
        self.status = "Возвращено положение, которое было при запуске".into();
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

    fn handle_keys(&mut self, ctx: &egui::Context) {
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
        if self.drag.is_none() && t.elapsed() >= LIVE_DELAY {
            if self.has_changes() {
                self.apply();
            } else {
                self.changed_at = None;
            }
        } else {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
    }

    fn save_config(&mut self) {
        self.config.anchor = self.anchor;
        self.config.live = self.live;
        self.config.save();
    }

    // ---------------------------------------------------------------- toolbar

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let changes = self.has_changes();
            if ui
                .add_enabled(changes, egui::Button::new("✔ Применить"))
                .on_hover_text("Записать новое положение мониторов в Windows (Enter)")
                .clicked()
            {
                self.apply();
            }
            if ui.add_enabled(changes, egui::Button::new("Сбросить правки")).clicked() {
                self.pos = self.displays.iter().map(|d| (d.x, d.y)).collect();
                self.changed_at = None;
            }
            if ui
                .button("Как было при запуске")
                .on_hover_text("Вернуть раскладку, которая была до запуска программы")
                .clicked()
            {
                self.revert_to_initial();
            }
            if ui.button("⟳ Обновить").on_hover_text("Перечитать мониторы из Windows").clicked() {
                self.reload();
                self.fit_view = true;
                self.status = format!("Найдено мониторов: {}", self.displays.len());
            }
            if ui.button("Вписать").on_hover_text("Показать всю раскладку").clicked() {
                self.fit_view = true;
            }

            ui.separator();
            if ui
                .checkbox(&mut self.live, "Применять сразу")
                .on_hover_text("Каждый сдвиг сразу применяется в Windows")
                .changed()
            {
                self.save_config();
            }
            if ui
                .checkbox(&mut self.rulers, "Линейки на экранах")
                .on_hover_text("Показать миллиметровые линейки вдоль стыков на самих мониторах")
                .changed()
            {
                self.push_overlay();
            }
            if ui.checkbox(&mut self.topmost, "Поверх окон").changed() {
                let level = if self.topmost {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                };
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            }

            ui.separator();
            ui.label("Ноль линеек:");
            let before = self.anchor;
            egui::ComboBox::from_id_salt("anchor")
                .selected_text(anchor_name(self.anchor))
                .show_ui(ui, |ui| {
                    for a in [Anchor::Start, Anchor::Center, Anchor::End, Anchor::Cursor] {
                        ui.selectable_value(&mut self.anchor, a, anchor_name(a));
                    }
                })
                .response
                .on_hover_text(
                    "Точка стыка, где курсор переходит без скачка. От неё отсчитываются миллиметры на линейках.",
                );
            if self.anchor != before {
                self.save_config();
                self.push_overlay();
            }
        });
    }

    // ---------------------------------------------------------------- side panel

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Мониторы");
        for i in 0..self.displays.len() {
            let d = &self.displays[i];
            let text = format!(
                "{}  {}{}",
                d.number,
                d.name,
                if d.primary { "  · основной" } else { "" }
            );
            if ui.selectable_label(self.selected == Some(i), text).clicked() {
                self.selected = Some(i);
            }
        }
        ui.separator();

        let Some(i) = self.selected.filter(|&i| i < self.displays.len()) else {
            ui.label("Выберите монитор");
            return;
        };
        self.monitor_details(ui, i);
        ui.separator();
        self.edge_details(ui, i);
    }

    fn monitor_details(&mut self, ui: &mut egui::Ui, i: usize) {
        let d = self.displays[i].clone();
        ui.heading(format!("Монитор {}", d.number));

        egui::Grid::new("details").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Режим");
            ui.label(format!("{}×{} @ {} Гц", d.width, d.height, d.refresh));
            ui.end_row();

            ui.label("Масштаб Windows");
            ui.label(format!("{:.0}%", d.dpi_scale * 100.0));
            ui.end_row();

            ui.label("Положение, пикс.");
            ui.horizontal(|ui| {
                let (mut x, mut y) = self.pos[i];
                let cx = ui.add(egui::DragValue::new(&mut x).prefix("X ").speed(1)).changed();
                let cy = ui.add(egui::DragValue::new(&mut y).prefix("Y ").speed(1)).changed();
                if cx || cy {
                    self.pos[i] = (x, y);
                    self.mark_changed();
                }
            });
            ui.end_row();

            let ((mut w, mut h), source) = self.sizes[i];
            ui.label("Экран, мм");
            ui.horizontal(|ui| {
                let cw = ui.add(egui::DragValue::new(&mut w).speed(0.1).range(50.0..=3000.0).max_decimals(1)).changed();
                ui.label("×");
                let ch = ui.add(egui::DragValue::new(&mut h).speed(0.1).range(50.0..=3000.0).max_decimals(1)).changed();
                if cw || ch {
                    self.sizes[i] = ((w, h), SizeSource::User);
                    self.config.sizes.insert(d.key(), [w, h]);
                    self.save_config();
                    self.push_overlay();
                }
            });
            ui.end_row();

            ui.label("");
            ui.horizontal(|ui| {
                ui.weak(match source {
                    SizeSource::Edid => "из EDID монитора",
                    SizeSource::User => "введено вручную",
                    SizeSource::Guess => "не определено, измерьте и введите",
                });
                if source == SizeSource::User && ui.small_button("сбросить").clicked() {
                    self.config.sizes.remove(&d.key());
                    self.save_config();
                    self.sizes[i] = self.size_for(&d);
                    self.push_overlay();
                }
            });
            ui.end_row();

            let (mw, mh) = self.mm(i);
            let diag = ((mw * mw + mh * mh).sqrt()) / 25.4;
            let ppi = d.width as f32 / (mw / 25.4);
            ui.label("Диагональ, плотность");
            ui.label(format!("{diag:.1}″, {ppi:.1} PPI ({:.3} мм/пикс.)", mw / d.width as f32));
            ui.end_row();
        });
        ui.label(egui::RichText::new(
            "EDID часто округляет размер. Для точной подгонки измерьте видимую область экрана линейкой.",
        ).small().weak());

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Шаг:");
            for (s, name) in [(Step::Px(1), "1 пикс."), (Step::Px(5), "5"), (Step::Px(10), "10"), (Step::Mm, "≈1 мм")] {
                ui.selectable_value(&mut self.step, s, name);
            }
        });
        let b = Vec2::splat(30.0);
        egui::Grid::new("nudge").spacing([4.0, 4.0]).show(ui, |ui| {
            ui.label("");
            if ui.add_sized(b, egui::Button::new("↑")).clicked() {
                self.nudge(i, 0, -1);
            }
            ui.label("");
            ui.end_row();
            if ui.add_sized(b, egui::Button::new("←")).clicked() {
                self.nudge(i, -1, 0);
            }
            ui.label("");
            if ui.add_sized(b, egui::Button::new("→")).clicked() {
                self.nudge(i, 1, 0);
            }
            ui.end_row();
            ui.label("");
            if ui.add_sized(b, egui::Button::new("↓")).clicked() {
                self.nudge(i, 0, 1);
            }
            ui.label("");
            ui.end_row();
        });
        ui.weak("Стрелки на клавиатуре сдвигают на шаг, с Shift на 10 шагов.");
    }

    fn edge_details(&mut self, ui: &mut egui::Ui, i: usize) {
        ui.heading("Стыки");
        let mons = self.edited_mons();
        let edges: Vec<Edge> = find_edges(&mons).into_iter().filter(|e| e.a == i || e.b == i).collect();
        if edges.is_empty() {
            ui.colored_label(Color32::from_rgb(230, 160, 60), "Монитор не касается других. Windows сам придвинет его при применении.");
            return;
        }
        for e in edges {
            let j = e.other(i);
            let me_b = e.b == i;
            let (side, start_word, low, high, presets) = match (e.axis, me_b) {
                (Axis::Vertical, true) => ("слева", "Верх", "ниже", "выше", ["верх", "центр", "низ"]),
                (Axis::Vertical, false) => ("справа", "Верх", "ниже", "выше", ["верх", "центр", "низ"]),
                (Axis::Horizontal, true) => ("сверху", "Левый край", "правее", "левее", ["лево", "центр", "право"]),
                (Axis::Horizontal, false) => ("снизу", "Левый край", "правее", "левее", ["лево", "центр", "право"]),
            };
            let other = self.displays[j].clone();
            ui.group(|ui| {
                ui.strong(format!("{side}: монитор {} {}", other.number, other.name));

                let t0 = anchor_t(&e, self.anchor, None);
                // Offset of my side's start relative to the neighbour's, in mm, positive = lower / further right.
                let d_ab = implied_offset_mm(&e, &mons, t0);
                let mine = if me_b { d_ab } else { -d_ab };
                let px_off = if me_b { e.start(&mons[e.b]) - e.start(&mons[e.a]) } else { e.start(&mons[e.a]) - e.start(&mons[e.b]) };
                ui.label(format!(
                    "{start_word} этого экрана {} края соседа на {:.1} мм ({:+} пикс.)",
                    if mine >= 0.0 { low } else { high },
                    mine.abs(),
                    px_off as i32
                ));
                let err = mismatch_mm(&e, &mons, t0, e.s0 as f64).abs().max(mismatch_mm(&e, &mons, t0, e.s1 as f64).abs());
                if err >= 0.05 {
                    ui.weak(format!(
                        "Плотность пикселей разная: курсор идёт без скачка в точке «{}», на концах стыка расхождение до {err:.1} мм.",
                        anchor_name(self.anchor).to_lowercase()
                    ));
                }

                let anchor = self.anchor;
                ui.horizontal(|ui| {
                    ui.label("Выровнять по краю:");
                    let (la, lb) = (e.len_mm(&mons[e.a]), e.len_mm(&mons[e.b]));
                    let d_values = [0.0, (la - lb) / 2.0, la - lb];
                    for (name, d) in presets.iter().zip(d_values) {
                        if ui.button(*name).on_hover_text("Считая, что края экранов стоят ровно, без смещения по высоте").clicked() {
                            self.move_to(i, &e, &mons, d, anchor, me_b);
                        }
                    }
                });

                let key = (self.displays[i].device_name.clone(), other.device_name.clone());
                let mut measured = *self.measured.get(&key).unwrap_or(&0.0);
                ui.horizontal(|ui| {
                    ui.label(format!("Измерено: {start_word} этого экрана {low} на"));
                    ui.add(egui::DragValue::new(&mut measured).speed(0.1).max_decimals(1).suffix(" мм"));
                    if ui
                        .button("Подогнать")
                        .on_hover_text(format!("Положите линейку поперёк стыка и измерьте, насколько этот экран {low} соседа (отрицательное значение, если {high})."))
                        .clicked()
                    {
                        let d = if me_b { measured as f64 } else { -measured as f64 };
                        self.move_to(i, &e, &mons, d, anchor, me_b);
                    }
                });
                self.measured.insert(key, measured);
            });
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

    fn fit(&mut self, area: egui::Rect) {
        let mons = self.edited_mons();
        if mons.is_empty() {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for m in &mons {
            x0 = x0.min(m.rect.x);
            y0 = y0.min(m.rect.y);
            x1 = x1.max(m.rect.right());
            y1 = y1.max(m.rect.bottom());
        }
        let margin = 40.0;
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
        egui::Rect::from_min_max(
            self.to_screen(r.x as f32, r.y as f32),
            self.to_screen(r.right() as f32, r.bottom() as f32),
        )
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
        if self.fit_view || (area.size() != self.last_canvas.size() && self.drag.is_none()) {
            self.fit(area);
            self.fit_view = false;
        }
        self.last_canvas = area;

        // Zoom around the cursor, pan with the right or middle button.
        if let Some(hp) = resp.hover_pos() {
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

        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = self.hit(p) {
                    self.selected = Some(i);
                    self.drag = Some(Drag { index: i, start_mouse: p, start_pos: self.pos[i] });
                }
            }
        }
        if let Some(d) = &self.drag {
            if let Some(p) = resp.interact_pointer_pos() {
                let delta = (p - d.start_mouse) / self.view_scale;
                let x = d.start_pos.0 as f64 + delta.x as f64;
                let y = d.start_pos.1 as f64 + delta.y as f64;
                let i = d.index;
                let no_snap = ui.input(|inp| inp.modifiers.alt);
                let new = if no_snap { (x.round() as i32, y.round() as i32) } else { self.snap(i, x, y) };
                if new != self.pos[i] {
                    self.pos[i] = new;
                    self.mark_changed();
                }
            }
            if !resp.dragged_by(PointerButton::Primary) {
                self.drag = None;
            }
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(i) = self.hit(p) {
                    self.selected = Some(i);
                }
            }
        }

        let painter = painter.with_clip_rect(area);
        painter.rect_filled(area, CornerRadius::ZERO, ui.visuals().extreme_bg_color);

        let mons = self.edited_mons();
        let applied = self.applied_mons();
        let accent = Color32::from_rgb(60, 160, 255);

        // Where monitors are right now in Windows, if they've been moved since.
        for (i, m) in applied.iter().enumerate() {
            if m.rect != mons[i].rect {
                painter.rect_stroke(
                    self.screen_rect(&m.rect),
                    CornerRadius::ZERO,
                    Stroke::new(1.0, Color32::from_gray(110)),
                    StrokeKind::Inside,
                );
            }
        }

        for (i, m) in mons.iter().enumerate() {
            let d = &self.displays[i];
            let r = self.screen_rect(&m.rect);
            let sel = self.selected == Some(i);
            let overlaps = mons.iter().enumerate().any(|(j, o)| j != i && o.rect.intersects(&m.rect));
            let fill = if sel { Color32::from_rgb(38, 62, 98) } else { Color32::from_rgb(48, 52, 60) };
            painter.rect_filled(r, CornerRadius::same(2), fill);
            let border = if overlaps {
                Color32::from_rgb(230, 80, 80)
            } else if sel {
                accent
            } else {
                Color32::from_gray(140)
            };
            painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(if sel { 2.0 } else { 1.0 }, border), StrokeKind::Inside);

            let big = (r.height() * 0.28).clamp(10.0, 64.0);
            painter.text(r.center() - Vec2::new(0.0, big * 0.35), Align2::CENTER_CENTER, d.number.to_string(), FontId::proportional(big), Color32::WHITE);
            if r.width() > 120.0 && r.height() > 70.0 {
                let (mw, mh) = self.mm(i);
                let info = format!("{}\n{}×{} · {:.0}×{:.0} мм\n({}, {})", d.name, d.width, d.height, mw, mh, m.rect.x, m.rect.y);
                painter.text(r.center() + Vec2::new(0.0, big * 0.45), Align2::CENTER_TOP, info, FontId::proportional(12.0), Color32::from_gray(200));
            }
        }

        // Shared edges with their pixel offset.
        for e in find_edges(&mons) {
            let (a, b) = match e.axis {
                Axis::Vertical => (self.to_screen(e.pos as f32, e.s0 as f32), self.to_screen(e.pos as f32, e.s1 as f32)),
                Axis::Horizontal => (self.to_screen(e.s0 as f32, e.pos as f32), self.to_screen(e.s1 as f32, e.pos as f32)),
            };
            painter.line_segment([a, b], Stroke::new(3.0, Color32::from_rgb(70, 210, 120)));
            let off = e.start(&mons[e.b]) - e.start(&mons[e.a]);
            let mid = a + (b - a) / 2.0;
            let t0 = anchor_t(&e, self.anchor, None);
            let mm = implied_offset_mm(&e, &mons, t0);
            painter.text(
                mid,
                Align2::CENTER_CENTER,
                format!("{:+} пикс. / {:+.1} мм", off as i32, mm),
                FontId::proportional(12.0),
                Color32::from_rgb(140, 240, 170),
            );
        }

        painter.text(
            area.left_bottom() + Vec2::new(8.0, -8.0),
            Align2::LEFT_BOTTOM,
            "Перетаскивайте мониторы мышью (Alt без прилипания) · колесо масштаб · правая кнопка двигает вид",
            FontId::proportional(12.0),
            Color32::from_gray(130),
        );
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);
        self.live_tick(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            let pending = if self.has_changes() { "  · есть неприменённые изменения" } else { "" };
            ui.label(format!("{}{pending}", self.status));
        });
        egui::Panel::right("side").resizable(true).default_size(380.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui));
        });
        egui::CentralPanel::default().show(ui, |ui| self.canvas(ui));
    }
}

fn anchor_name(a: Anchor) -> &'static str {
    match a {
        Anchor::Start => "Начало стыка",
        Anchor::Center => "Центр стыка",
        Anchor::End => "Конец стыка",
        Anchor::Cursor => "Под курсором",
    }
}

/// Uses Segoe UI from Windows, falling back to egui's bundled fonts.
fn setup_fonts(ctx: &egui::Context) {
    let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("segoe".into(), egui::FontData::from_owned(bytes).into());
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        list.insert(0, "segoe".into());
    }
    if let Ok(sym) = std::fs::read(r"C:\Windows\Fonts\seguisym.ttf") {
        fonts.font_data.insert("segoe-symbol".into(), egui::FontData::from_owned(sym).into());
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            list.insert(1, "segoe-symbol".into());
        }
    }
    ctx.set_fonts(fonts);
}
