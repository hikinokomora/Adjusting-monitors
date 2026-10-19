//! Look of the editor window: palette, fonts and a few custom widgets.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Response, RichText, Sense,
    Stroke, StrokeKind, TextStyle, Ui, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(12, 14, 19);
pub const PANEL: Color32 = Color32::from_rgb(18, 21, 28);
pub const CARD: Color32 = Color32::from_rgb(24, 28, 37);
pub const CARD_HI: Color32 = Color32::from_rgb(32, 37, 49);
pub const STROKE: Color32 = Color32::from_rgb(40, 46, 60);
pub const TEXT: Color32 = Color32::from_rgb(230, 233, 241);
pub const MUTED: Color32 = Color32::from_rgb(136, 145, 166);
pub const ACCENT: Color32 = Color32::from_rgb(91, 141, 255);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(36, 54, 98);
pub const GREEN: Color32 = Color32::from_rgb(64, 201, 138);
pub const AMBER: Color32 = Color32::from_rgb(242, 178, 72);
pub const RED: Color32 = Color32::from_rgb(240, 96, 96);

/// Segoe MDL2 Assets glyphs, rendered through the font fallback chain.
pub mod icon {
    pub const CHECK: &str = "\u{E73E}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const UNDO: &str = "\u{E7A7}";
    pub const HISTORY: &str = "\u{E81C}";
    pub const FIT: &str = "\u{E740}";
    pub const RULER: &str = "\u{ED5E}";
    pub const UP: &str = "\u{E70E}";
    pub const DOWN: &str = "\u{E70D}";
    pub const LEFT: &str = "\u{E76B}";
    pub const RIGHT: &str = "\u{E76C}";
}

pub fn setup(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, path: &str, pos: usize| {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), egui::FontData::from_owned(bytes).into());
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.insert(pos.min(list.len()), name.into());
                }
            }
        }
    };
    add("segoe", r"C:\Windows\Fonts\segoeui.ttf", 0);
    add("icons", r"C:\Windows\Fonts\segmdl2.ttf", 1);
    add("symbols", r"C:\Windows\Fonts\seguisym.ttf", 2);
    let mut strong_list = fonts.families[&FontFamily::Proportional].clone();
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf") {
        fonts.font_data.insert("segoe-sb".into(), egui::FontData::from_owned(bytes).into());
        strong_list.insert(0, "segoe-sb".into());
    }
    fonts.families.insert(FontFamily::Name("strong".into()), strong_list);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(20.0, FontFamily::Name("strong".into()))),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.window_margin = Margin::same(12);

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.extreme_bg_color = BG;
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.window_corner_radius = CornerRadius::same(12);
        v.window_stroke = Stroke::new(1.0, STROKE);
        let r = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.weak_bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, STROKE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = CARD_HI;
        w.inactive.weak_bg_fill = CARD_HI;
        w.inactive.bg_stroke = Stroke::new(1.0, STROKE);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = Color32::from_rgb(42, 49, 65);
        w.hovered.weak_bg_fill = Color32::from_rgb(42, 49, 65);
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(62, 72, 94));
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = r;
        w.active.bg_fill = ACCENT_DIM;
        w.active.weak_bg_fill = ACCENT_DIM;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.corner_radius = r;
        w.open = w.active;
    });
}

/// A rounded panel that groups related controls.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(11.0).color(MUTED).extra_letter_spacing(0.6));
}

pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).family(FontFamily::Name("strong".into()))
}

/// Filled accent button for the main action.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let fill = if enabled { ACCENT } else { CARD_HI };
    let color = if enabled { Color32::WHITE } else { MUTED };
    ui.add_enabled(
        enabled,
        egui::Button::new(strong(text).color(color))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(Vec2::new(0.0, 34.0)),
    )
}

/// Quiet button without a background until hovered.
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(TEXT))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, STROKE))
            .corner_radius(CornerRadius::same(8))
            .min_size(Vec2::new(0.0, 34.0)),
    )
}

/// iOS-style switch with a label.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let size = Vec2::new(34.0, 20.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(14.0), TEXT);
    let total = Vec2::new(size.x + 8.0 + galley.size().x, size.y.max(galley.size().y));
    let (rect, mut resp) = ui.allocate_exact_size(total, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let track = egui::Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - size.y / 2.0), size);
    let fill = lerp_color(CARD_HI, ACCENT, t);
    let p = ui.painter();
    p.rect(track, CornerRadius::same(10), fill, Stroke::new(1.0, if *on { ACCENT } else { STROKE }), StrokeKind::Inside);
    let knob_x = egui::lerp(track.left() + 10.0..=track.right() - 10.0, t);
    p.circle_filled(Pos2::new(knob_x, track.center().y), 7.0, Color32::WHITE);
    p.galley(Pos2::new(track.right() + 8.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Row of mutually exclusive options. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (v, name) in options {
                    let sel = *value == *v;
                    let text = if sel { strong(*name).color(Color32::WHITE) } else { RichText::new(*name).color(MUTED) };
                    let b = egui::Button::new(text)
                        .fill(if sel { ACCENT } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(7))
                        .min_size(Vec2::new(0.0, 26.0));
                    if ui.add(b).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Small label/value pair inside a tinted box.
pub fn stat(ui: &mut Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(CARD_HI)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(label).size(11.0).color(MUTED));
                ui.label(strong(value).size(14.0));
            });
        });
}

/// Rounded status pill with a coloured dot.
pub fn pill(ui: &mut Ui, color: Color32, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(13))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, color);
                ui.label(RichText::new(text).size(13.0).color(color));
            });
        });
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
