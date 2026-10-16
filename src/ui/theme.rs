use eframe::egui::{self, Color32, CornerRadius, Stroke};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub accent: Color32,
    /// Headings and the text of selected items.
    pub strong_text: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub muted: Color32,
    pub background: Color32,
    pub sidebar_fill: Color32,
    pub card_fill: Color32,
    pub card_stroke: Color32,
}

/// Neutral greys with one plain blue for links, focus and progress.
const DARK: Palette = Palette {
    accent: Color32::from_rgb(92, 150, 220),
    strong_text: Color32::from_gray(236),
    success: Color32::from_rgb(98, 178, 120),
    warning: Color32::from_rgb(214, 168, 72),
    danger: Color32::from_rgb(222, 98, 92),
    muted: Color32::from_gray(150),
    background: Color32::from_gray(30),
    sidebar_fill: Color32::from_gray(23),
    card_fill: Color32::from_gray(38),
    card_stroke: Color32::from_gray(54),
};

const LIGHT: Palette = Palette {
    accent: Color32::from_rgb(30, 98, 178),
    strong_text: Color32::from_gray(20),
    success: Color32::from_rgb(40, 138, 78),
    warning: Color32::from_rgb(170, 116, 18),
    danger: Color32::from_rgb(190, 56, 50),
    muted: Color32::from_gray(108),
    background: Color32::from_gray(244),
    sidebar_fill: Color32::from_gray(233),
    card_fill: Color32::WHITE,
    card_stroke: Color32::from_gray(216),
};

pub fn palette(ctx: &egui::Context) -> Palette {
    if ctx.global_style().visuals.dark_mode { DARK } else { LIGHT }
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, styled_visuals(egui::Visuals::dark(), &DARK));
    ctx.set_visuals_of(egui::Theme::Light, styled_visuals(egui::Visuals::light(), &LIGHT));
    ctx.style_mut_of(egui::Theme::Dark, style_spacing);
    ctx.style_mut_of(egui::Theme::Light, style_spacing);
}

fn style_spacing(style: &mut egui::Style) {
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.indent = 18.0;
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(22.0));
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.5));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.5));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
}

fn styled_visuals(mut visuals: egui::Visuals, palette: &Palette) -> egui::Visuals {
    let dark = visuals.dark_mode;
    // Picks the dark or the light variant of a grey.
    let grey = |dark_value: u8, light_value: u8| Color32::from_gray(if dark { dark_value } else { light_value });
    let rounding = CornerRadius::same(4);
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.card_fill;
    visuals.extreme_bg_color = grey(25, 248);
    visuals.faint_bg_color = grey(44, 238);
    visuals.selection.bg_fill = palette.accent.gamma_multiply(if dark { 0.45 } else { 0.25 });
    visuals.selection.stroke = Stroke::new(1.0, palette.strong_text);
    visuals.hyperlink_color = palette.accent;
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(6);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = rounding;
    }
    visuals.widgets.inactive.weak_bg_fill = grey(50, 226);
    visuals.widgets.hovered.weak_bg_fill = grey(62, 214);
    visuals.widgets.active.weak_bg_fill = grey(72, 204);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, grey(84, 186));
    visuals
}

/// Outlined container used for jobs and settings groups.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let palette = palette(ui.ctx());
    egui::Frame::new()
        .fill(palette.card_fill)
        .stroke(Stroke::new(1.0, palette.card_stroke))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(14))
}

pub fn banner(_ui: &egui::Ui, color: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.7)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 10))
}
