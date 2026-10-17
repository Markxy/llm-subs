use eframe::egui::{self, RichText, Ui};

use crate::app::{DownloadStatus, FfmpegStatus};
use crate::ffmpeg::{DownloadPhase, DownloadProgress};
use crate::ui::{theme, widgets, LlmSubsApp};

pub fn show(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    widgets::page_title(ui, "Setup", "Startup behaviour, speed, where LLM Subs keeps its files, and FFmpeg.");

    widgets::section_title(ui, "Startup and window");
    theme::card(ui).show(ui, |ui| {
        let mut run_on_startup = app.run_on_startup;
        if ui.checkbox(&mut run_on_startup, "Start LLM Subs when I log in").changed() {
            match crate::platform::set_run_on_startup(run_on_startup) {
                Ok(()) => app.run_on_startup = run_on_startup,
                Err(error) => app.toast(format!("{error:#}")),
            }
        }
        widgets::hint(ui, "Starts hidden in the tray, so watched folders are handled without a window in the way.");
        if ui.checkbox(&mut app.draft.general.start_hidden, "Start hidden when launched manually").changed() {
            app.mark_dirty();
        }
        let tray_available = app.tray_available();
        if ui.add_enabled(tray_available, egui::Checkbox::new(&mut app.draft.general.close_to_tray, "Closing the window keeps LLM Subs running in the tray")).changed() {
            app.mark_dirty();
        }
        if !tray_available {
            widgets::hint(ui, "No tray icon is available in this build or desktop environment, so closing the window quits.");
        }
    });

    ui.add_space(14.0);
    widgets::section_title(ui, "Performance");
    theme::card(ui).show(ui, |ui| show_performance(app, ui));

    ui.add_space(14.0);
    widgets::section_title(ui, "Files");
    theme::card(ui).show(ui, |ui| {
        let paths = app.runtime().paths().clone();
        let rows = [
            ("Configuration", paths.config_file(), "Every setting, including fine-tuning not shown in this window; edits are picked up live."),
            ("Job history", paths.history_file(), ""),
            ("Logs", paths.log_dir(), ""),
            ("Managed FFmpeg", paths.managed_ffmpeg_dir(), ""),
        ];
        let mut failure = None;
        for (label, path, description) in rows {
            widgets::form_row(ui, label, |ui| {
                widgets::trailing_row(
                    ui,
                    |ui| {
                        if ui.small_button("Open").clicked()
                            && let Err(error) = crate::platform::reveal_in_folder(&path)
                        {
                            failure = Some(format!("{error:#}"));
                        }
                    },
                    |ui| widgets::truncated(ui, RichText::new(path.display().to_string()).small().color(palette.muted)),
                );
            });
            if !description.is_empty() {
                widgets::hint(ui, description);
            }
        }
        if let Some(message) = failure {
            app.toast(message);
        }
    });

    ui.add_space(14.0);
    show_ffmpeg(app, ui);

    ui.add_space(14.0);
    widgets::hint(ui, &format!("LLM Subs {} · MIT licensed · uses FFmpeg for all media handling", env!("CARGO_PKG_VERSION")));
}

/// Characters a request may carry per subtitle cue; the default 100 cues fill 10,000.
const CHARS_PER_CUE: usize = 100;

fn show_performance(app: &mut LlmSubsApp, ui: &mut Ui) {
    let mut changed = false;
    widgets::form_row(ui, "Max concurrent jobs", |ui| {
        changed |= ui.add(egui::Slider::new(&mut app.draft.general.max_concurrent_jobs, 1..=4)).changed();
    });
    widgets::hint(ui, "Files translated at the same time. Each one already sends several requests at once.");
    let translation = &mut app.draft.translation;
    widgets::form_row(ui, "Max parallel requests", |ui| {
        changed |= ui.add(egui::Slider::new(&mut translation.parallel_requests, 1..=10)).changed();
    });
    widgets::hint(ui, "Requests in flight per file. With 1, each batch sees the previous batch's translations; set 1 for local servers that answer one request at a time.");
    widgets::form_row(ui, "Subtitle cues per request", |ui| {
        if ui.add(egui::Slider::new(&mut translation.lines_per_request, 10..=1000).logarithmic(true)).changed() {
            // The character budget would otherwise cut batches shorter than the slider says.
            translation.max_chars_per_request = translation.max_chars_per_request.max(translation.lines_per_request * CHARS_PER_CUE);
            changed = true;
        }
    });
    widgets::hint(ui, "Larger batches give the model more of the film at once; lower it for small local models that lose track of long lists.");
    if changed {
        app.mark_dirty();
    }
}

fn show_ffmpeg(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    widgets::section_title(ui, "FFmpeg");
    theme::card(ui).show(ui, |ui| {
        match app.runtime().ffmpeg_status() {
            FfmpegStatus::Available { tools, version } => {
                ui.horizontal(|ui| {
                    widgets::status_dot(ui, palette.success);
                    ui.label(RichText::new(format!("Found via {}", tools.origin.label())).strong());
                });
                widgets::hint(ui, &tools.ffmpeg.display().to_string());
                if let Some(version) = version {
                    widgets::hint(ui, &version);
                }
            }
            FfmpegStatus::Missing => {
                ui.horizontal(|ui| {
                    widgets::status_dot(ui, palette.danger);
                    ui.label(RichText::new("Not found").strong());
                });
                widgets::hint(ui, "Install FFmpeg, point LLM Subs at an existing copy below, or download a build for this app only.");
            }
        }
        ui.add_space(8.0);
        widgets::form_row(ui, "Use FFmpeg from", |ui| {
            let mut text = app.draft.ffmpeg.path_override.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            let width = widgets::field_width(ui, 360.0, 100.0);
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(width).hint_text("folder containing ffmpeg and ffprobe")).changed() {
                app.draft.ffmpeg.path_override = (!text.trim().is_empty()).then(|| std::path::PathBuf::from(text.trim()));
                app.mark_dirty();
            }
            if ui.button("Browse…").clicked() {
                app.file_dialogs.pick_folder(|app, folder| {
                    app.draft.ffmpeg.path_override = Some(folder);
                    app.mark_dirty();
                });
            }
        });
        ui.horizontal(|ui| {
            match app.runtime().download_status() {
                DownloadStatus::Running(progress) => download_progress_bar(ui, &progress, 300.0),
                DownloadStatus::Failed(error) => {
                    if ui.button("Retry download").clicked() {
                        app.runtime().start_ffmpeg_download();
                    }
                    ui.add(egui::Label::new(RichText::new(error).small().color(palette.danger)).wrap());
                }
                DownloadStatus::Idle | DownloadStatus::Finished => {
                    let label = if matches!(app.runtime().ffmpeg_status(), FfmpegStatus::Missing) { "Download FFmpeg for LLM Subs" } else { "Download a fresh managed build" };
                    if ui.button(label).on_hover_text("Static builds from BtbN (Windows), johnvansickle.com (Linux) and evermeet.cx (macOS), stored in the LLM Subs data folder.").clicked() {
                        app.runtime().start_ffmpeg_download();
                    }
                    if ui.button("Re-detect").clicked() {
                        app.runtime().refresh_ffmpeg();
                    }
                }
            }
        });
    });
}

pub(crate) fn download_progress_bar(ui: &mut Ui, progress: &DownloadProgress, width: f32) {
    let fraction = progress.total_bytes.map(|total| progress.downloaded_bytes as f32 / total.max(1) as f32).unwrap_or(0.0);
    let text = match progress.phase {
        DownloadPhase::Downloading => match progress.total_bytes {
            Some(total) => format!("Downloading {} of {}", widgets::format_bytes(progress.downloaded_bytes), widgets::format_bytes(total)),
            None => format!("Downloading {}", widgets::format_bytes(progress.downloaded_bytes)),
        },
        DownloadPhase::Extracting => "Unpacking…".to_string(),
        DownloadPhase::Finished => "Done".to_string(),
    };
    ui.add(egui::ProgressBar::new(fraction).desired_width(width).text(text).animate(progress.total_bytes.is_none()));
}
