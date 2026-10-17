use eframe::egui::{self, RichText, Ui};

use crate::app::FfmpegStatus;
use crate::jobs::{JobRecord, JobStatus, LogLevel};
use crate::llm::chatgpt::MANAGE_USAGE_URL;
use crate::llm::{format_token_count, Cost};
use crate::watch::PendingFile;
use crate::ui::views::View;
use crate::ui::{theme, widgets, LlmSubsApp};

pub fn show(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    widgets::trailing_row(
        ui,
        |ui| {
            if ui.button("Add folder…").clicked() {
                app.file_dialogs.pick_folder(|app, folder| app.queue_paths(vec![folder], false));
            }
            if ui.button("Add files…").clicked() {
                app.file_dialogs.pick_media_files(&app.draft.watch, |app, files| app.queue_paths(files, false));
            }
            let watching_paused = app.runtime().is_watch_paused();
            if ui.selectable_label(watching_paused, if watching_paused { "Watching paused" } else { "Pause watching" }).clicked() {
                app.runtime().set_watch_paused(!watching_paused);
            }
            let translations_paused = app.runtime().jobs().is_paused();
            let label = if translations_paused { "Translations paused" } else { "Pause translations" };
            if ui.selectable_label(translations_paused, label).on_hover_text("Running jobs go back to the queue and start over on resume").clicked() {
                app.runtime().jobs().set_paused(!translations_paused);
            }
        },
        |ui| widgets::truncated(ui, RichText::new("Jobs").heading()),
    );
    widgets::hint(ui, "Drop media or subtitle files anywhere in this window to translate them.");
    ui.add_space(8.0);

    show_banners(app, ui);

    let records = std::mem::take(&mut app.jobs_cache);
    let (active, finished): (Vec<&JobRecord>, Vec<&JobRecord>) = records.iter().partition(|r| r.status.is_active());
    let pending = app.runtime().pending_files();

    if records.is_empty() && pending.is_empty() {
        theme::card(ui).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new("Nothing here yet").size(17.0).strong());
                ui.label(RichText::new("Add a watched folder, or drop a video here to translate its subtitles.").color(palette.muted));
                ui.add_space(24.0);
            });
        });
    }

    if !pending.is_empty() {
        waiting_files(ui, &pending);
        ui.add_space(10.0);
    }

    if !active.is_empty() {
        widgets::section_title(ui, "In progress");
        for record in &active {
            active_job_card(app, ui, record);
            ui.add_space(6.0);
        }
        ui.add_space(10.0);
    }

    if !finished.is_empty() {
        ui.horizontal(|ui| {
            widgets::section_title(ui, "History");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear history").clicked() {
                    app.runtime().jobs().clear_history();
                    app.selected_job = None;
                }
            });
        });
        for record in &finished {
            history_row(app, ui, record);
        }
    }
    app.jobs_cache = records;
}

fn show_banners(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    if app.runtime().jobs().is_paused() {
        theme::banner(ui, palette.warning).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Translations are paused.").strong());
                ui.label("New files still join the queue and start once you resume.");
                if ui.link("Resume").clicked() {
                    app.runtime().jobs().set_paused(false);
                }
            });
        });
        ui.add_space(6.0);
    }
    if matches!(app.runtime().ffmpeg_status(), FfmpegStatus::Missing) {
        theme::banner(ui, palette.warning).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("⚠").color(palette.warning));
                ui.label("FFmpeg was not found. Media files cannot be inspected until it is installed or downloaded.");
                if ui.link("Open Setup").clicked() {
                    app.switch_view(View::Setup);
                }
            });
        });
        ui.add_space(6.0);
    }
    let problems = app.runtime().validation_problems(&app.draft);
    if !problems.is_empty() {
        theme::banner(ui, palette.warning).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("⚠").color(palette.warning));
                ui.label(RichText::new("Translation is not configured yet:").strong());
                ui.label(problems.join(" "));
                if ui.link("Configure models").clicked() {
                    app.switch_view(View::Models);
                }
            });
        });
        ui.add_space(6.0);
    }
}

/// Files in watched folders that are still downloading or being copied, so not queued yet.
fn waiting_files(ui: &mut Ui, pending: &[PendingFile]) {
    const SHOWN: usize = 5;
    let palette = theme::palette(ui.ctx());
    widgets::section_title(ui, "Waiting for files to finish");
    theme::card(ui).show(ui, |ui| {
        for (index, file) in pending.iter().take(SHOWN).enumerate() {
            if index > 0 {
                ui.separator();
            }
            let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.path.display().to_string());
            widgets::trailing_row(
                ui,
                |ui| {
                    ui.label(RichText::new(widgets::format_duration(chrono::Utc::now() - file.since)).color(palette.muted));
                },
                |ui| {
                    widgets::status_chip(ui, "Waiting", palette.muted);
                    widgets::truncated(ui, RichText::new(name).strong())
                },
            );
            let size = if file.size > 0 { format!(" · {}", widgets::format_bytes(file.size)) } else { String::new() };
            widgets::hint(ui, &format!("{}{size}", file.state.describe()));
        }
        if pending.len() > SHOWN {
            widgets::hint(ui, &format!("and {} more", pending.len() - SHOWN));
        }
    });
}

fn active_job_card(app: &mut LlmSubsApp, ui: &mut Ui, record: &JobRecord) {
    let palette = theme::palette(ui.ctx());
    theme::card(ui).show(ui, |ui| {
        widgets::trailing_row(
            ui,
            |ui| {
                if ui.button("Cancel").clicked() {
                    app.runtime().jobs().cancel(&record.id);
                }
                widgets::status_chip(ui, record.status.label(), palette.accent);
            },
            |ui| widgets::truncated(ui, RichText::new(record.input.file_name()).strong().size(15.0)),
        );
        widgets::hint(ui, &record.input.path().display().to_string());
        ui.add_space(4.0);
        ui.add(egui::ProgressBar::new(record.progress).desired_height(8.0).fill(palette.accent).corner_radius(egui::CornerRadius::same(4)));
        widgets::trailing_row(
            ui,
            |ui| {
                if let Some(duration) = record.duration() {
                    ui.label(RichText::new(widgets::format_duration(duration)).color(palette.muted));
                }
            },
            |ui| widgets::truncated(ui, RichText::new(format!("{} · {}", record.stage.label(), record.activity)).color(palette.muted)),
        );
        let selected = app.selected_job.as_deref() == Some(record.id.as_str());
        if ui.small_button(if selected { "Hide log" } else { "Show log" }).clicked() {
            app.selected_job = if selected { None } else { Some(record.id.clone()) };
        }
        if selected {
            log_view(ui, record);
        }
    });
}

fn history_row(app: &mut LlmSubsApp, ui: &mut Ui, record: &JobRecord) {
    let palette = theme::palette(ui.ctx());
    let selected = app.selected_job.as_deref() == Some(record.id.as_str());
    let (chip_color, summary) = match &record.status {
        JobStatus::Succeeded => (
            palette.success,
            format!(
                "{} cues translated{}",
                record.translated_cues,
                if record.untranslated_cues > 0 { format!(", {} kept in source language", record.untranslated_cues) } else { String::new() }
            ),
        ),
        JobStatus::Skipped { reason, .. } => (palette.muted, reason.clone()),
        JobStatus::Failed { error } => (palette.danger, error.clone()),
        JobStatus::Cancelled => (palette.warning, "Cancelled".to_string()),
        JobStatus::Queued | JobStatus::Running => (palette.accent, record.activity.clone()),
    };

    let hit_chatgpt_limit = matches!(&record.status, JobStatus::Failed { error } if error.contains(MANAGE_USAGE_URL));
    let frame = theme::card(ui).inner_margin(egui::Margin::symmetric(14, 10));
    let response = frame
        .show(ui, |ui| {
            widgets::trailing_row(
                ui,
                |ui| {
                    ui.label(RichText::new(record.finished_at.unwrap_or(record.created_at).with_timezone(&chrono::Local).format("%b %d, %H:%M").to_string()).small().color(palette.muted));
                    if let Some(cost) = &record.cost {
                        let text = match cost {
                            Cost::Usd(_) => cost.describe(),
                            Cost::ChatGptPlan => "ChatGPT plan".to_string(),
                        };
                        ui.label(RichText::new(text).small().color(palette.muted));
                    }
                    if record.usage.total() > 0 {
                        ui.label(RichText::new(format!("{} tokens", format_token_count(record.usage.total()))).small().color(palette.muted));
                    }
                    if let Some(duration) = record.duration() {
                        ui.label(RichText::new(widgets::format_duration(duration)).small().color(palette.muted));
                    }
                },
                |ui| {
                    widgets::status_chip(ui, record.status.label(), chip_color);
                    ui.vertical(|ui| {
                        widgets::truncated(ui, RichText::new(record.input.file_name()).strong());
                        // An error can be long: one line while collapsed, all of it when opened.
                        let summary = RichText::new(&summary).small().color(palette.muted);
                        if selected {
                            ui.label(summary);
                        } else {
                            widgets::truncated(ui, summary);
                        }
                    });
                },
            );
            if selected {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    widgets::hint(ui, &record.input.path().display().to_string());
                });
                if let Some(source) = &record.source_description {
                    widgets::hint(ui, &format!("Source: {source}"));
                }
                if record.usage.total() > 0 {
                    let cost = record.cost.map(|cost| format!(" · {}", cost.describe())).unwrap_or_default();
                    widgets::hint(ui, &format!("{} requests · {}{cost}", record.requests, record.usage.describe()));
                }
                ui.horizontal(|ui| {
                    if let Some(output) = &record.output_path
                        && output.exists()
                        && ui.small_button("Show output").clicked()
                        && let Err(error) = crate::platform::reveal_in_folder(output)
                    {
                        app.toast(format!("{error:#}"));
                    }
                    if hit_chatgpt_limit
                        && ui.small_button("Manage ChatGPT usage").clicked()
                        && let Err(error) = app.runtime().open_chatgpt_usage_page()
                    {
                        app.toast(format!("{error:#}"));
                    }
                    if ui.small_button("Translate again").clicked() {
                        app.queue_paths(vec![record.input.path().to_path_buf()], true);
                    }
                    if ui.small_button("Remove").clicked() {
                        app.runtime().jobs().remove_from_history(&record.id);
                        app.selected_job = None;
                    }
                });
                log_view(ui, record);
            }
        })
        .response;
    if response.interact(egui::Sense::click()).clicked() {
        app.selected_job = if selected { None } else { Some(record.id.clone()) };
    }
    ui.add_space(4.0);
}

fn log_view(ui: &mut Ui, record: &JobRecord) {
    let palette = theme::palette(ui.ctx());
    if record.log.is_empty() {
        return;
    }
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(&record.id).max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &record.log {
                    let color = match line.level {
                        LogLevel::Info => palette.muted,
                        LogLevel::Warning => palette.warning,
                        LogLevel::Error => palette.danger,
                    };
                    ui.label(
                        RichText::new(format!("{}  {}", line.at.with_timezone(&chrono::Local).format("%H:%M:%S"), line.message))
                            .small()
                            .monospace()
                            .color(color),
                    );
                }
            });
        });
}
