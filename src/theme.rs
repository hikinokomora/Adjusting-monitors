//! Look of the editor window: palette, fonts and a few custom widgets.
//! Plain and flat on purpose: neutral greys, one accent colour, thin borders.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, TextStyle, Ui, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(24, 24, 27);
pub const PANEL: Color32 = Color32::from_rgb(32, 32, 36);
pub const CARD: Color32 = Color32::from_rgb(39, 39, 44);
pub const CARD_HI: Color32 = Color32::from_rgb(50, 50, 56);
pub const STROKE: Color32 = Color32::from_rgb(56, 56, 63);
pub const TEXT: Color32 = Color32::from_rgb(228, 228, 232);
pub const MUTED: Color32 = Color32::from_rgb(150, 150, 160);
pub const ACCENT: Color32 = Color32::from_rgb(70, 136, 240);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(40, 62, 100);
pub const GREEN: Color32 = Color32::from_rgb(80, 196, 130);
pub const AMBER: Color32 = Color32::from_rgb(232, 176, 80);
pub const RED: Color32 = Color32::from_rgb(232, 96, 96);

pub const RADIUS: u8 = 6;

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
    pub const ZOOM_IN: &str = "\u{E8A3}";
    pub const ZOOM_OUT: &str = "\u{E71F}";
}

pub fn strong_family() -> FontFamily {
    FontFamily::Name("strong".into())
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
    fonts.families.insert(strong_family(), strong_list);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, strong_family())),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 5.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.window_margin = Margin::same(12);
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;

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
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.window_stroke = Stroke::new(1.0, STROKE);
        let r = CornerRadius::same(RADIUS);
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
        w.hovered.bg_fill = Color32::from_rgb(60, 60, 68);
        w.hovered.weak_bg_fill = Color32::from_rgb(60, 60, 68);
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(78, 78, 88));
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = ACCENT_DIM;
        w.active.weak_bg_fill = ACCENT_DIM;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;
        w.open = w.active;
    });
}

// ---------------------------------------------------------------- containers

/// A bordered block that groups related controls under a short title.
pub fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            caption(ui, title);
            add(ui)
        })
        .inner
}

/// Opaque panel for controls placed over the canvas.
pub fn floating<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| ui.horizontal(add).inner)
        .inner
}

pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(strong(text).size(13.0).color(MUTED));
}

pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).family(strong_family())
}

// ---------------------------------------------------------------- widgets

/// Filled button for the main action.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let (fill, color, stroke) = if enabled {
        (ACCENT, Color32::WHITE, Stroke::NONE)
    } else {
        (CARD, MUTED, Stroke::new(1.0, STROKE))
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(strong(text).color(color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

/// Outlined secondary button.
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(TEXT))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, STROKE))
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(32.0, 32.0)),
    )
}

/// Small square icon button for the canvas panels.
pub fn icon_button(ui: &mut Ui, glyph: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(glyph).size(13.0))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(28.0, 28.0)),
    )
}

/// Switch with a label.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let size = Vec2::new(32.0, 18.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(14.0), TEXT);
    let total = Vec2::new(size.x + 8.0 + galley.size().x, 28.0);
    let (rect, mut resp) = ui.allocate_exact_size(total, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let track = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - size.y / 2.0), size);
    let p = ui.painter();
    let fill = lerp_color(CARD_HI, ACCENT, t);
    let stroke = if *on { ACCENT } else { Color32::from_rgb(90, 90, 100) };
    p.rect(track, CornerRadius::same(9), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let knob_x = egui::lerp(track.left() + 9.0..=track.right() - 9.0, t);
    p.circle_filled(Pos2::new(knob_x, track.center().y), 5.5, if *on { Color32::WHITE } else { MUTED });
    p.galley(Pos2::new(track.right() + 8.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Row of mutually exclusive options. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (v, name) in options {
                    let sel = *value == *v;
                    let galley = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(13.5), TEXT);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 26.0), Sense::click());
                    let p = ui.painter();
                    let color = if sel {
                        p.rect_filled(rect, CornerRadius::same(RADIUS - 2), ACCENT);
                        Color32::WHITE
                    } else if resp.hovered() {
                        p.rect_filled(rect, CornerRadius::same(RADIUS - 2), CARD_HI);
                        TEXT
                    } else {
                        MUTED
                    };
                    p.galley(rect.center() - galley.size() / 2.0, galley, color);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Label above a value.
pub fn stat(ui: &mut Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(label).size(12.0).color(MUTED));
        ui.label(strong(value).size(15.0));
    });
}

/// Status text with a coloured dot.
pub fn status(ui: &mut Ui, color: Color32, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), color);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 16.0, 28.0), Sense::hover());
    let p = ui.painter();
    p.circle_filled(Pos2::new(rect.left() + 4.0, rect.center().y), 4.0, color);
    p.galley(Pos2::new(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
