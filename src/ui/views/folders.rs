use eframe::egui::{RichText, Ui};

use crate::config::WatchedFolder;
use crate::ui::{theme, widgets, LlmSubsApp};

pub fn show(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    widgets::page_title(ui, "Watched folders", "New media files that appear in these folders are translated automatically.");

    if ui.checkbox(&mut app.draft.watch.enabled, "Watch folders for new files").changed() {
        app.mark_dirty();
    }
    ui.add_space(8.0);

    let mut remove_index = None;
    let folder_count = app.draft.watch.folders.len();
    for index in 0..folder_count {
        theme::card(ui).show(ui, |ui| {
            let folder = &mut app.draft.watch.folders[index];
            widgets::trailing_row(
                ui,
                |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove_index = Some(index);
                    }
                    if !folder.path.is_dir() {
                        widgets::status_chip(ui, "not found", palette.warning);
                    }
                },
                |ui| widgets::truncated(ui, RichText::new(folder.path.display().to_string()).strong()),
            );
            ui.horizontal(|ui| {
                let mut changed = ui.checkbox(&mut folder.recursive, "Include subfolders").changed();
                changed |= ui
                    .checkbox(&mut folder.process_existing, "Also translate files already in the folder")
                    .on_hover_text("Otherwise only files added while LLM Subs runs are picked up.")
                    .changed();
                if changed {
                    app.draft_dirty = true;
                    app.last_edit = Some(std::time::Instant::now());
                }
            });
        });
        ui.add_space(6.0);
    }
    if let Some(index) = remove_index {
        app.draft.watch.folders.remove(index);
        app.mark_dirty();
    }

    if ui.button("Add folder…").clicked()
        && let Some(path) = widgets::pick_folder()
    {
        if app.draft.watch.folders.iter().any(|f| f.path == path) {
            app.toast("That folder is already watched");
        } else {
            app.draft.watch.folders.push(WatchedFolder { path, ..WatchedFolder::default() });
            app.mark_dirty();
        }
    }
}
