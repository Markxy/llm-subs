use eframe::egui::{self, Ui};

use crate::config::OutputMode;
use crate::ui::{theme, widgets, LlmSubsApp};

pub fn show(app: &mut LlmSubsApp, ui: &mut Ui) {
    widgets::page_title(ui, "Translation", "What to translate into, where the result goes, and how the translator should write.");

    widgets::section_title(ui, "Target");
    theme::card(ui).show(ui, |ui| {
        widgets::form_row(ui, "Translate into", |ui| {
            let mut code = app.draft.general.target_language.clone();
            if widgets::language_picker(ui, "target-language", &mut code, &mut app.language_filter) {
                app.draft.general.target_language = code;
                app.mark_dirty();
            }
        });
        widgets::form_row(ui, "Prefer source subtitles in", |ui| {
            let width = widgets::field_width(ui, 360.0, 0.0);
            if widgets::string_list(ui, "preferred-source", &mut app.draft.translation.preferred_source_languages, "en, ja (empty = the audio's language)", width) {
                app.mark_dirty();
            }
        });
        if ui.checkbox(&mut app.draft.translation.skip_if_target_exists, "Skip media that already has subtitles in the target language").changed() {
            app.mark_dirty();
        }
        widgets::hint(ui, "When off, it is translated anyway and an earlier translation next to it is replaced.");
    });

    ui.add_space(14.0);
    widgets::section_title(ui, "Output");
    theme::card(ui).show(ui, |ui| {
        let mut mode = app.draft.output.mode;
        if ui.radio_value(&mut mode, OutputMode::Sidecar, "Subtitle file next to the media (Movie.de.srt)").changed() {
            app.draft.output.mode = mode;
            app.mark_dirty();
        }
        widgets::hint(ui, "Recommended. Every player and media server loads it automatically and the media file is never rewritten.");
        if ui.radio_value(&mut mode, OutputMode::Embed, "Add a subtitle track inside the media file").changed() {
            app.draft.output.mode = mode;
            app.mark_dirty();
        }
        widgets::hint(ui, "Matroska and MP4 only. The file is remuxed without re-encoding, which still rewrites it once.");
        if app.draft.output.mode == OutputMode::Embed {
            ui.add_space(4.0);
            if ui.checkbox(&mut app.draft.output.keep_sidecar_when_embedding, "Keep the .srt file as well").changed() {
                app.mark_dirty();
            }
        }
    });

    ui.add_space(14.0);
    widgets::section_title(ui, "Style");
    theme::card(ui).show(ui, |ui| {
        ui.label("Additional instructions");
        widgets::hint(ui, "Optional. Tone, names and forms of address are already handled; add only preferences of your own.");
        if ui
            .add(egui::TextEdit::multiline(&mut app.draft.translation.extra_instructions).desired_rows(4).desired_width(f32::INFINITY).hint_text("e.g. Tone down strong swearing. Keep Japanese honorifics such as -san and -sama."))
            .changed()
        {
            app.mark_dirty();
        }
        ui.add_space(6.0);
        widgets::form_row(ui, "Max characters per line", |ui| {
            if ui.add(egui::Slider::new(&mut app.draft.translation.line_wrap.max_line_length, 24..=64)).changed() {
                app.mark_dirty();
            }
        });
    });
}
