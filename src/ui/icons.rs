//! Outline icons from [Bootstrap Icons](https://icons.getbootstrap.com), named as they are
//! there. They are glyphs of a small font, so they sit in any text and take its colour. The
//! font holds only the icons listed here: after adding one (its code point is on the icon's
//! page), run `assets/subset_icons.py`.

use std::sync::Arc;

use eframe::egui;

const FONT_NAME: &str = "bootstrap-icons";

pub const ARROW_CLOCKWISE: &str = "\u{f116}";
pub const ARROW_REPEAT: &str = "\u{f130}";
pub const BAR_CHART: &str = "\u{f17e}";
pub const BOX_ARROW_RIGHT: &str = "\u{f1c3}";
pub const BOX_ARROW_UP_RIGHT: &str = "\u{f1c5}";
pub const CHECK_CIRCLE: &str = "\u{f26b}";
pub const CHEVRON_DOWN: &str = "\u{f282}";
pub const CHEVRON_UP: &str = "\u{f286}";
pub const CLOCK: &str = "\u{f293}";
pub const CPU: &str = "\u{f2d6}";
pub const DASH_CIRCLE: &str = "\u{f2e6}";
pub const DOWNLOAD: &str = "\u{f30a}";
pub const EXCLAMATION_TRIANGLE: &str = "\u{f33b}";
pub const EYE: &str = "\u{f341}";
pub const EYE_SLASH: &str = "\u{f340}";
pub const FILE_EARMARK: &str = "\u{f392}";
pub const FOLDER: &str = "\u{f3d7}";
pub const FOLDER2_OPEN: &str = "\u{f3d8}";
pub const GEAR: &str = "\u{f3e5}";
pub const INBOX: &str = "\u{f42d}";
pub const LIST_TASK: &str = "\u{f477}";
pub const PAUSE_CIRCLE: &str = "\u{f4c2}";
pub const PLUG: &str = "\u{f4f7}";
pub const PLUS_LG: &str = "\u{f64d}";
pub const SEARCH: &str = "\u{f52a}";
pub const SLASH_CIRCLE: &str = "\u{f567}";
pub const TRANSLATE: &str = "\u{f658}";
pub const TRASH: &str = "\u{f5de}";
pub const X_CIRCLE: &str = "\u{f623}";
pub const X_LG: &str = "\u{f659}";

/// An icon in front of a label.
pub fn labeled(icon: &str, label: &str) -> String {
    format!("{icon}  {label}")
}

/// Makes the icons available in the default text font.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(FONT_NAME.to_owned(), font_data());
    let family = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    // Right after the main font: line height comes from the first font in the list, and
    // egui's own emoji font uses some of the same code points for other symbols.
    family.insert(family.len().min(1), FONT_NAME.to_owned());
    ctx.set_fonts(fonts);
}

fn font_data() -> Arc<egui::FontData> {
    Arc::new(egui::FontData::from_static(include_bytes!("../../assets/bootstrap-icons-subset.ttf")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A constant added without rebuilding the font would show up as an empty box.
    #[test]
    fn the_font_has_every_icon() {
        let listed: Vec<char> = include_str!("icons.rs")
            .lines()
            .filter(|line| line.starts_with("pub const ") && line.contains(": &str = \"\\u{"))
            .map(|line| {
                let hex = line.split("\\u{").nth(1).and_then(|rest| rest.split('}').next()).unwrap();
                char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()
            })
            .collect();
        assert!(listed.len() > 20, "found only {} icons", listed.len());

        let mut fonts = egui::FontDefinitions::empty();
        fonts.font_data.insert(FONT_NAME.to_owned(), font_data());
        fonts.families.insert(egui::FontFamily::Proportional, vec![FONT_NAME.to_owned()]);
        let mut fonts = egui::epaint::text::Fonts::new(egui::epaint::text::TextOptions::default(), fonts);
        let mut fonts = fonts.with_pixels_per_point(1.0);
        let font_id = egui::FontId::proportional(14.0);
        assert!(!fonts.has_glyph(&font_id, 'a'), "the check cannot tell what is missing");
        for icon in listed {
            assert!(fonts.has_glyph(&font_id, icon), "U+{:X} is not in the font", icon as u32);
        }
    }
}
