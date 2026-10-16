//! Small reusable pieces shared by the views.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, Ui};

use crate::config::{all_languages, find_language};

use super::theme;

pub const LABEL_WIDTH: f32 = 190.0;

pub fn page_title(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.heading(title);
    if !subtitle.is_empty() {
        let palette = theme::palette(ui.ctx());
        ui.label(RichText::new(subtitle).color(palette.muted));
    }
    ui.add_space(10.0);
}

pub fn section_title(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).size(16.0).strong());
    ui.add_space(2.0);
}

pub fn hint(ui: &mut Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(text).small().color(palette.muted));
}

pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

pub fn status_chip(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color).strong());
        });
}

/// Label on the left, editor on the right, aligned across rows.
pub fn form_row<R>(ui: &mut Ui, label: &str, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_WIDTH, ui.spacing().interact_size.y], egui::Label::new(label).halign(egui::Align::LEFT));
        add_contents(ui)
    })
    .inner
}

/// A row whose `trailing` widgets keep their place at the right edge; they are added right
/// to left. `leading` gets only the width left over, so long text there has to truncate
/// (see [`truncated`]) instead of pushing the trailing widgets out of the window.
pub fn trailing_row<R>(ui: &mut Ui, trailing: impl FnOnce(&mut Ui), leading: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            trailing(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), leading).inner
        })
        .inner
    })
    .inner
}

/// `preferred` shrunk to the width left in this row minus `reserve` for what follows on the
/// same line. One overflowing field widens the whole page, which pushes every right-aligned
/// widget below it out of view, so fixed-width fields squeeze in narrow windows instead.
pub fn field_width(ui: &Ui, preferred: f32, reserve: f32) -> f32 {
    preferred.min(ui.available_width() - reserve).max(60.0)
}

/// One line of text cut off with "…" where the space ends; the full text shows on hover.
pub fn truncated(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// Text field for an optional number; empty means "not set".
pub fn optional_u32(ui: &mut Ui, value: &mut Option<u32>, hint_text: &str) -> bool {
    let id = ui.id().with("optional_u32").with(hint_text);
    let mut text = ui.data_mut(|data| data.get_temp::<String>(id)).unwrap_or_else(|| value.map(|v| v.to_string()).unwrap_or_default());
    let response = ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint_text).desired_width(110.0));
    if !response.has_focus() {
        text = value.map(|v| v.to_string()).unwrap_or_default();
    }
    let mut changed = false;
    if response.changed() {
        let parsed = text.trim().parse::<u32>().ok();
        if text.trim().is_empty() || parsed.is_some() {
            *value = parsed;
            changed = true;
        }
    }
    ui.data_mut(|data| data.insert_temp(id, text));
    changed
}

pub fn optional_f32(ui: &mut Ui, value: &mut Option<f32>, hint_text: &str) -> bool {
    let id = ui.id().with("optional_f32").with(hint_text);
    let mut text = ui.data_mut(|data| data.get_temp::<String>(id)).unwrap_or_else(|| value.map(|v| v.to_string()).unwrap_or_default());
    let response = ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint_text).desired_width(110.0));
    if !response.has_focus() {
        text = value.map(|v| v.to_string()).unwrap_or_default();
    }
    let mut changed = false;
    if response.changed() {
        let parsed = text.trim().parse::<f32>().ok();
        if text.trim().is_empty() || parsed.is_some() {
            *value = parsed;
            changed = true;
        }
    }
    ui.data_mut(|data| data.insert_temp(id, text));
    changed
}

/// Comma-separated editor for a list of short strings such as language codes.
pub fn string_list(ui: &mut Ui, id_salt: &str, items: &mut Vec<String>, hint_text: &str, width: f32) -> bool {
    let id = ui.id().with(id_salt);
    let joined = items.join(", ");
    let mut text = ui.data_mut(|data| data.get_temp::<String>(id)).unwrap_or_else(|| joined.clone());
    let response = ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint_text).desired_width(width));
    if !response.has_focus() {
        text = joined;
    }
    let mut changed = false;
    if response.changed() {
        *items = text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        changed = true;
    }
    ui.data_mut(|data| data.insert_temp(id, text));
    changed
}

/// Searchable dropdown over the language table. `code` holds the selected code.
/// The filter field takes focus when the dropdown opens; Enter picks the first match.
pub fn language_picker(ui: &mut Ui, id_salt: &str, code: &mut String, filter: &mut String) -> bool {
    let selected_text = find_language(code).map(|l| l.display_name()).unwrap_or_else(|| if code.is_empty() { "Select…".to_string() } else { code.clone() });
    let filter_id = ui.make_persistent_id(id_salt).with("filter");
    let mut changed = false;
    // The default closes on any click, including the one into the filter field.
    let combo = egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_text)
        .width(field_width(ui, 300.0, 0.0))
        .truncate()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let filter_response = ui.add(egui::TextEdit::singleline(filter).id(filter_id).hint_text("Type to filter").desired_width(f32::INFINITY));
            let pick_first = filter_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.separator();
            let needle = filter.to_lowercase();
            let mut matches = all_languages()
                .iter()
                .filter(|l| needle.is_empty() || format!("{} {} {}", l.english_name, l.native_name, l.code).to_lowercase().contains(&needle))
                .peekable();
            let mut picked = if pick_first { matches.peek().map(|l| l.code) } else { None };
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for language in matches {
                    if ui.selectable_label(language.code == code, language.display_name()).clicked() {
                        picked = Some(language.code);
                    }
                }
            });
            if let Some(picked) = picked {
                *code = picked.to_string();
                changed = true;
                ui.close();
            }
        });
    // Just opened: start from the full list with the cursor in the filter field.
    if combo.response.clicked() && combo.inner.is_some() {
        filter.clear();
        ui.memory_mut(|mem| mem.request_focus(filter_id));
    }
    changed
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

pub fn format_duration(duration: chrono::Duration) -> String {
    let seconds = duration.num_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// A figure with a caption above it and optional details below.
pub fn stat_tile(ui: &mut Ui, label: &str, value: &str, details: &str) {
    let palette = theme::palette(ui.ctx());
    theme::card(ui).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(label).small().color(palette.muted));
        ui.label(RichText::new(value).size(22.0).strong());
        for line in details.lines() {
            ui.label(RichText::new(line).small().color(palette.muted));
        }
    });
}

pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Choose a folder").pick_folder()
}

pub fn pick_media_files(media_extensions: &[String], subtitle_extensions: &[String]) -> Vec<PathBuf> {
    let media: Vec<&str> = media_extensions.iter().map(String::as_str).collect();
    let subtitles: Vec<&str> = subtitle_extensions.iter().map(String::as_str).collect();
    rfd::FileDialog::new()
        .set_title("Choose media or subtitle files")
        .add_filter("Media files", &media)
        .add_filter("Subtitle files", &subtitles)
        .pick_files()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_duration(chrono::Duration::seconds(75)), "1m 15s");
    }
}
