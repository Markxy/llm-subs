//! Desktop window built with egui. The window is a thin client of [`AppRuntime`]: it
//! edits a draft of the configuration, shows job records and forwards actions.

mod file_dialog;
mod icon;
mod notifications;
mod onboarding;
mod theme;
mod tray;
mod views;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;

use crate::app::{AppRuntime, ProviderTest};
use crate::config::{Config, ProviderKind};
use crate::jobs::{JobOptions, JobOrigin, JobRecord};

use file_dialog::FileDialogs;
use tray::{TrayCommand, TrayController, TrayState};
use views::usage::UsagePeriod;
use views::View;

const AUTOSAVE_DELAY: Duration = Duration::from_millis(700);
const TOAST_LIFETIME: Duration = Duration::from_secs(4);
/// Smallest window the layout still fits in, in points.
const MIN_WINDOW_SIZE: (f64, f64) = (800.0, 520.0);

pub fn run(runtime: Arc<AppRuntime>, start_hidden: bool) -> Result<()> {
    register_app_icon(&runtime);
    let window_icon = icon::render(64);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LLM Subs")
            .with_app_id(crate::platform::APP_ID)
            .with_inner_size([980.0, 680.0])
            .with_visible(!start_hidden)
            .with_icon(Arc::new(egui::IconData { rgba: window_icon.rgba, width: window_icon.width, height: window_icon.height })),
        ..Default::default()
    };
    eframe::run_native(
        "LLM Subs",
        options,
        Box::new(move |creation_context| Ok(Box::new(LlmSubsApp::new(creation_context, runtime)))),
    )
    .map_err(|error| anyhow::anyhow!("window error: {error}"))
}

/// Saves the icon for the places that want an image file: Windows notifications and the
/// Linux desktop entry. Done before the window opens so the desktop can match it at once.
fn register_app_icon(runtime: &AppRuntime) {
    let path = runtime.paths().data_dir().join("icon.png");
    match icon::write_png(&icon::render(256), &path) {
        Ok(()) => {
            crate::platform::notifications::register(&path);
            crate::platform::install_desktop_entry(&path);
        }
        Err(error) => tracing::warn!(error = %format!("{error:#}"), "could not write the app icon"),
    }
}

struct Toast {
    message: String,
    shown_at: Instant,
}

pub struct LlmSubsApp {
    runtime: Arc<AppRuntime>,
    view: View,
    draft: Config,
    draft_dirty: bool,
    last_edit: Option<Instant>,
    last_seen_config: Arc<Config>,
    jobs_cache: Vec<JobRecord>,
    jobs_changed: Arc<AtomicBool>,
    selected_job: Option<String>,
    toasts: Vec<Toast>,
    tray: Option<TrayController>,
    quitting: bool,
    provider_tests: HashMap<String, ProviderTest>,
    language_filter: String,
    run_on_startup: bool,
    show_api_keys: bool,
    usage_period: UsagePeriod,
    /// ChatGPT providers whose model list was fetched automatically already.
    requested_model_lists: HashSet<String>,
    announcer: notifications::JobAnnouncer,
    /// The first-run guide, while it is open.
    onboarding: Option<onboarding::Onboarding>,
    file_dialogs: FileDialogs,
}

impl LlmSubsApp {
    fn new(creation_context: &eframe::CreationContext<'_>, runtime: Arc<AppRuntime>) -> Self {
        theme::apply(&creation_context.egui_ctx);
        // Set on winit directly: egui turns a minimum size into pixels once, for the monitor
        // the window opens on, which fights every move to a monitor with other scaling.
        if let Some(window) = creation_context.winit_window() {
            window.set_min_inner_size(Some(winit::dpi::LogicalSize::new(MIN_WINDOW_SIZE.0, MIN_WINDOW_SIZE.1)));
            crate::platform::follow_suggested_rect_on_scale_change(window);
        }

        let jobs_changed = Arc::new(AtomicBool::new(true));
        let notifier_ctx = creation_context.egui_ctx.clone();
        let notifier_flag = jobs_changed.clone();
        runtime.set_ui_notifier(Arc::new(move || {
            notifier_flag.store(true, Ordering::Relaxed);
            notifier_ctx.request_repaint();
        }));

        let tray = TrayController::start(creation_context.egui_ctx.clone(), icon::render(32));
        let config = runtime.config();
        let mut app = Self {
            draft: (*config).clone(),
            last_seen_config: config,
            runtime,
            view: View::Jobs,
            draft_dirty: false,
            last_edit: None,
            jobs_cache: Vec::new(),
            jobs_changed,
            selected_job: None,
            toasts: Vec::new(),
            tray,
            quitting: false,
            provider_tests: HashMap::new(),
            language_filter: String::new(),
            run_on_startup: crate::platform::is_run_on_startup_enabled(),
            show_api_keys: false,
            usage_period: UsagePeriod::default(),
            requested_model_lists: HashSet::new(),
            announcer: notifications::JobAnnouncer::default(),
            onboarding: None,
            file_dialogs: FileDialogs::default(),
        };
        if !app.draft.general.onboarding_complete {
            if app.runtime.validation_problems(&app.draft).is_empty() {
                // Set up by hand or before the guide existed.
                app.draft.general.onboarding_complete = true;
                app.mark_dirty();
            } else {
                app.onboarding = Some(onboarding::Onboarding::default());
            }
        }
        app
    }

    pub(crate) fn runtime(&self) -> &Arc<AppRuntime> {
        &self.runtime
    }

    /// Call after any edit of `self.draft`; the change is saved shortly after.
    pub(crate) fn mark_dirty(&mut self) {
        self.draft_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    pub(crate) fn toast(&mut self, message: impl Into<String>) {
        self.toasts.push(Toast { message: message.into(), shown_at: Instant::now() });
    }

    pub(crate) fn queue_paths(&mut self, paths: Vec<PathBuf>, force: bool) {
        if paths.is_empty() {
            return;
        }
        let queued = self.runtime.submit_paths(&paths, JobOrigin::Manual, JobOptions { force }).len();
        let message = match queued {
            0 => "Nothing new to translate in the dropped files".to_string(),
            1 => "Queued 1 file".to_string(),
            n => format!("Queued {n} files"),
        };
        self.toast(message);
        self.view = View::Jobs;
    }

    pub(crate) fn switch_view(&mut self, view: View) {
        self.view = view;
    }

    pub(crate) fn tray_available(&self) -> bool {
        self.tray.is_some()
    }

    fn refresh_jobs_if_needed(&mut self) {
        if self.jobs_changed.swap(false, Ordering::Relaxed) {
            self.jobs_cache = self.runtime.jobs().snapshot();
            self.announcer.update(&self.jobs_cache);
        }
    }

    fn sync_config_from_runtime(&mut self) {
        let current = self.runtime.config();
        if Arc::ptr_eq(&current, &self.last_seen_config) {
            return;
        }
        self.last_seen_config = current.clone();
        if !self.draft_dirty {
            self.draft = (*current).clone();
        }
    }

    fn autosave_if_due(&mut self, ctx: &egui::Context) {
        if !self.draft_dirty {
            return;
        }
        let due = self.last_edit.is_none_or(|edited| edited.elapsed() >= AUTOSAVE_DELAY);
        if !due {
            ctx.request_repaint_after(AUTOSAVE_DELAY);
            return;
        }
        match self.runtime.save_config(self.draft.clone()) {
            Ok(()) => {
                self.draft_dirty = false;
                self.last_seen_config = self.runtime.config();
            }
            Err(error) => {
                self.draft_dirty = false;
                self.toast(format!("Could not save settings: {error:#}"));
            }
        }
    }

    fn handle_tray_commands(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        for command in tray.poll() {
            match command {
                TrayCommand::ShowWindow => show_window(ctx),
                TrayCommand::TogglePauseWatching => {
                    let paused = !self.runtime.is_watch_paused();
                    self.runtime.set_watch_paused(paused);
                }
                TrayCommand::TogglePauseTranslations => {
                    let jobs = self.runtime.jobs();
                    jobs.set_paused(!jobs.is_paused());
                }
                TrayCommand::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        tray.set_state(TrayState {
            watching_paused: self.runtime.is_watch_paused(),
            translations_paused: self.runtime.jobs().is_paused(),
            active_jobs: self.jobs_cache.iter().filter(|j| j.status.is_active()).count(),
        });
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) || self.quitting {
            return;
        }
        if self.draft.general.close_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| input.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.queue_paths(dropped, false);
        }
    }

    fn show_drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            return;
        }
        let palette = theme::palette(ctx);
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-overlay")));
        painter.rect_filled(screen, 0.0, palette.accent.gamma_multiply(0.18));
        painter.rect_stroke(screen.shrink(12.0), 12.0, egui::Stroke::new(2.0, palette.accent), egui::StrokeKind::Inside);
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            "Drop media or subtitle files to translate them",
            egui::FontId::proportional(22.0),
            palette.accent,
        );
    }

    /// Shown once after a ChatGPT account is connected, as ChatGPT asks apps to do.
    fn show_plan_notice(&mut self, ctx: &egui::Context) {
        let accounts = &self.runtime.llm().chatgpt;
        let Some(provider) = self
            .draft
            .providers
            .iter()
            .filter(|p| p.kind == ProviderKind::ChatGpt)
            .find(|p| accounts.account(&p.name).is_some_and(|account| !account.plan_notice_acknowledged))
            .map(|p| p.name.clone())
        else {
            return;
        };
        egui::Modal::new(egui::Id::new("chatgpt-plan-notice")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("You're using your ChatGPT plan");
            ui.add_space(4.0);
            ui.label("Eligible usage in this app uses your ChatGPT plan. Manage usage in your ChatGPT settings.");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Got it").clicked() {
                    accounts.acknowledge_plan_notice(&provider);
                }
                if ui.link("Manage usage").clicked()
                    && let Err(error) = self.runtime.open_chatgpt_usage_page()
                {
                    tracing::warn!(error = %format!("{error:#}"), "could not open the usage page");
                }
            });
        });
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|toast| toast.shown_at.elapsed() < TOAST_LIFETIME);
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for toast in &self.toasts {
                    theme::card(ui).show(ui, |ui| {
                        ui.label(&toast.message);
                    });
                    ui.add_space(6.0);
                }
            });
    }

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        egui::Panel::left("navigation")
            .exact_size(188.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(palette.sidebar_fill).inner_margin(egui::Margin::symmetric(14, 18)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new("LLM Subs").size(21.0).strong().color(palette.strong_text));
                });
                ui.add_space(2.0);
                ui.label(egui::RichText::new("subtitle translator").small().color(palette.muted));
                ui.add_space(18.0);

                for view in View::ALL {
                    let selected = self.view == view;
                    let text = egui::RichText::new(view.label()).size(15.0);
                    let response = ui.add_sized([ui.available_width(), 32.0], egui::Button::selectable(selected, text));
                    if response.clicked() {
                        self.view = view;
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(4.0);
                    let running = self.jobs_cache.iter().filter(|j| j.status.is_active()).count();
                    let watching = self.draft.watch.enabled && !self.draft.watch.folders.is_empty();
                    let paused = self.runtime.is_watch_paused();
                    let (dot, text) = if paused {
                        (palette.warning, "Watching paused".to_string())
                    } else if watching {
                        (palette.success, format!("Watching {} folder{}", self.draft.watch.folders.len(), if self.draft.watch.folders.len() == 1 { "" } else { "s" }))
                    } else {
                        (palette.muted, "Not watching".to_string())
                    };
                    ui.horizontal(|ui| {
                        widgets::status_dot(ui, dot);
                        ui.label(egui::RichText::new(text).small().color(palette.muted));
                    });
                    if self.runtime.jobs().is_paused() {
                        ui.horizontal(|ui| {
                            widgets::status_dot(ui, palette.warning);
                            ui.label(egui::RichText::new("Translations paused").small().color(palette.muted));
                        });
                    }
                    if running > 0 {
                        ui.horizontal(|ui| {
                            widgets::status_dot(ui, palette.accent);
                            ui.label(egui::RichText::new(format!("{running} job{} active", if running == 1 { "" } else { "s" })).small().color(palette.muted));
                        });
                    }
                    let waiting = self.runtime.pending_files().len();
                    if waiting > 0 {
                        ui.horizontal(|ui| {
                            widgets::status_dot(ui, palette.muted);
                            ui.label(egui::RichText::new(format!("{waiting} file{} still arriving", if waiting == 1 { "" } else { "s" })).small().color(palette.muted));
                        });
                    }
                    if self.draft_dirty {
                        ui.label(egui::RichText::new("Saving…").small().color(palette.muted));
                    }
                });
            });
    }
}

impl eframe::App for LlmSubsApp {
    /// Runs before every frame and also while the window is hidden, so tray commands
    /// and job updates are handled even when nothing is drawn.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_jobs_if_needed();
        self.sync_config_from_runtime();
        self.handle_tray_commands(ctx);
        if let Some((paths, on_choice)) = self.file_dialogs.take_choice() {
            on_choice(self, paths);
        }
        self.autosave_if_due(ctx);
        if self.jobs_cache.iter().any(|j| j.status.is_active()) {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else if !self.runtime.pending_files().is_empty() {
            // Keeps the waiting time and size of files still downloading current.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.handle_close_request(&ctx);

        self.show_sidebar(ui);
        let palette = theme::palette(&ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.background).inner_margin(egui::Margin::symmetric(26, 22)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    match self.view {
                        View::Jobs => views::jobs::show(self, ui),
                        View::Folders => views::folders::show(self, ui),
                        View::Translation => views::translation::show(self, ui),
                        View::Models => views::models::show(self, ui),
                        View::Usage => views::usage::show(self, ui),
                        View::Setup => views::setup::show(self, ui),
                    }
                });
            });

        self.show_drop_overlay(&ctx);
        if self.onboarding.is_some() {
            onboarding::show(self, &ctx);
        } else {
            self.show_plan_notice(&ctx);
        }
        self.show_toasts(&ctx);
        self.file_dialogs.open_requested(frame, &ctx, &self.runtime.tokio_handle());
    }
}

fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}
