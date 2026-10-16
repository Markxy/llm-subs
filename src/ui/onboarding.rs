//! First-run guide: connect a model (a ChatGPT sign-in, preferably) and choose the
//! language to translate into. FFmpeg is downloaded meanwhile when none was found.

use eframe::egui::{self, RichText, Ui};

use crate::app::{ChatGptSignIn, DownloadStatus, FfmpegStatus};
use crate::config::{Config, ModelSelection, ProviderKind, ProviderProfile};
use crate::llm::chatgpt::{ModelCatalog, SignInProgress, DEFAULT_MODEL};

use super::views::models::{chatgpt_button, manage_usage_link};
use super::views::setup::download_progress_bar;
use super::{theme, widgets, LlmSubsApp};

const WIDTH: f32 = 470.0;
const STEPS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Step {
    #[default]
    Provider,
    Language,
}

enum Action {
    Stay,
    GoTo(Step),
    Close,
}

/// An API key or a local server, the alternative to a ChatGPT sign-in.
struct ApiForm {
    kind: ProviderKind,
    base_url: String,
    api_key: String,
    model: String,
}

impl ApiForm {
    fn new(kind: ProviderKind) -> Self {
        let model = match kind {
            ProviderKind::OpenAi => "gpt-5",
            ProviderKind::Anthropic => "claude-opus-5-5",
            ProviderKind::ChatGpt => DEFAULT_MODEL,
            ProviderKind::OpenAiCompatible => "",
        };
        Self { kind, base_url: kind.default_base_url().to_string(), api_key: String::new(), model: model.to_string() }
    }

    fn is_complete(&self) -> bool {
        let has_key = self.kind == ProviderKind::OpenAiCompatible || !self.api_key.trim().is_empty();
        has_key && !self.model.trim().is_empty() && !self.base_url.trim().is_empty()
    }
}

#[derive(Default)]
pub struct Onboarding {
    step: Step,
    api_form: Option<ApiForm>,
    /// Provider the API form added, replaced instead of duplicated when the form is sent again.
    api_provider: Option<String>,
    ffmpeg_checked: bool,
}

pub fn show(app: &mut LlmSubsApp, ctx: &egui::Context) {
    let Some(mut guide) = app.onboarding.take() else { return };
    if !guide.ffmpeg_checked {
        guide.ffmpeg_checked = true;
        if matches!(app.runtime().ffmpeg_status(), FfmpegStatus::Missing) {
            app.runtime().start_ffmpeg_download();
        }
    }
    let mut action = Action::Stay;
    egui::Modal::new(egui::Id::new("onboarding")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.heading("Welcome to LLM Subs");
        action = match guide.step {
            Step::Provider => provider_step(app, &mut guide, ui),
            Step::Language => language_step(app, ui),
        };
        ui.add_space(6.0);
        ui.separator();
        ffmpeg_status(app, ui);
    });
    match action {
        Action::Stay => app.onboarding = Some(guide),
        Action::GoTo(step) => {
            guide.step = step;
            app.onboarding = Some(guide);
        }
        Action::Close => {
            app.draft.general.onboarding_complete = true;
            app.mark_dirty();
        }
    }
}

fn step_caption(ui: &mut Ui, number: usize, title: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(RichText::new(format!("Step {number} of {STEPS} · {title}")).color(palette.muted));
    ui.add_space(6.0);
}

fn provider_step(app: &mut LlmSubsApp, guide: &mut Onboarding, ui: &mut Ui) -> Action {
    step_caption(ui, 1, "Connect a model");
    if guide.api_form.is_some() {
        return api_step(app, guide, ui);
    }
    let ready = chatgpt_panel(app, ui);
    ui.add_space(8.0);
    let (mut proceed, mut later) = (false, false);
    widgets::trailing_row(
        ui,
        |ui| {
            if ready.is_some() {
                proceed = ui.button("Continue").clicked();
            } else {
                later = ui.link("Set up later").on_hover_text("Providers can be added under Models at any time.").clicked();
            }
        },
        |ui| {
            if ui.link("Use an API key or a local server instead").clicked() {
                guide.api_form = Some(ApiForm::new(ProviderKind::OpenAi));
            }
        },
    );
    match ready {
        Some(provider) if proceed => {
            app.runtime().llm().chatgpt.acknowledge_plan_notice(&provider);
            Action::GoTo(Step::Language)
        }
        _ if later => Action::Close,
        _ => Action::Stay,
    }
}

/// The ChatGPT provider the guide signs in: the configured one, or the default name when
/// that already has an account (signed in from the command line, say).
fn chatgpt_provider_name(app: &LlmSubsApp) -> Option<String> {
    app.draft
        .providers
        .iter()
        .find(|p| p.kind == ProviderKind::ChatGpt)
        .map(|p| p.name.clone())
        .or_else(|| app.runtime().llm().chatgpt.account("chatgpt").is_some().then(|| "chatgpt".to_string()))
}

/// Makes `name` a configured ChatGPT provider and the translator. Returns whether anything changed.
fn use_chatgpt_provider(draft: &mut Config, name: &str) -> bool {
    let mut changed = false;
    if draft.provider(name).is_none() {
        draft.providers.push(ProviderProfile {
            name: name.to_string(),
            kind: ProviderKind::ChatGpt,
            base_url: ProviderKind::ChatGpt.default_base_url().to_string(),
            ..ProviderProfile::default()
        });
        changed = true;
    }
    if draft.models.translation.provider != name {
        draft.models.translation = ModelSelection { provider: name.to_string(), model: DEFAULT_MODEL.to_string(), ..ModelSelection::default() };
        changed = true;
    }
    changed
}

/// Sign-in and model choice. Returns the provider once it can translate.
fn chatgpt_panel(app: &mut LlmSubsApp, ui: &mut Ui) -> Option<String> {
    let palette = theme::palette(ui.ctx());
    let runtime = app.runtime().clone();
    let name = chatgpt_provider_name(app);
    let view = name.as_deref().map(|name| runtime.llm().chatgpt.view(name)).unwrap_or_default();

    if let (Some(name), SignInProgress::Waiting { authorize_url }) = (&name, &view.sign_in) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Finish signing in in your browser…");
        });
        ui.horizontal(|ui| {
            if !authorize_url.is_empty() {
                if ui.small_button("Open the page again").clicked()
                    && let Err(error) = open::that_detached(authorize_url)
                {
                    tracing::warn!(%error, "could not open the browser");
                }
                if ui.small_button("Copy link").clicked() {
                    ui.ctx().copy_text(authorize_url.clone());
                }
            }
            if ui.small_button("Cancel").clicked() {
                runtime.chatgpt_cancel_sign_in(name);
            }
        });
        return None;
    }

    let (Some(name), Some(account)) = (name, view.account) else {
        ui.label("LLM Subs translates with a large language model. Connecting your ChatGPT plan is the quickest way: no API key to manage, and translations count toward the plan's usage limits.");
        ui.add_space(6.0);
        if let SignInProgress::Failed(error) = &view.sign_in {
            ui.add(egui::Label::new(RichText::new(error).small().color(palette.danger)).wrap());
        }
        if chatgpt_button(ui).clicked() {
            let name = chatgpt_provider_name(app).unwrap_or_else(|| unique_provider_name(&app.draft, "chatgpt"));
            if use_chatgpt_provider(&mut app.draft, &name) {
                app.mark_dirty();
            }
            runtime.chatgpt_sign_in(&name, ChatGptSignIn::Connect);
        }
        widgets::hint(ui, "Works with ChatGPT Plus and Pro. Sign in with ChatGPT is offered for personal and open-source use.");
        return None;
    };

    if use_chatgpt_provider(&mut app.draft, &name) {
        app.mark_dirty();
    }
    ui.horizontal(|ui| {
        widgets::status_dot(ui, if account.usage_sharing { palette.success } else { palette.warning });
        widgets::truncated(ui, RichText::new(format!("Signed in as {}", account.display_name())).strong());
    });
    if !account.usage_sharing {
        theme::banner(ui, palette.warning).show(ui, |ui| {
            ui.label("Usage sharing is turned off, so LLM Subs cannot translate with this plan.");
            if ui.button("Enable usage sharing").clicked() {
                runtime.chatgpt_sign_in(&name, ChatGptSignIn::EnableUsageSharing);
            }
        });
        return None;
    }

    if view.models == ModelCatalog::NotLoaded && app.requested_model_lists.insert(name.clone()) {
        runtime.chatgpt_load_models(&name);
    }
    let selection = &mut app.draft.models.translation;
    let mut changed = false;
    if let ModelCatalog::Loaded(models) = &view.models
        && !models.iter().any(|m| m.slug == selection.model)
        && let Some(fallback) = models.iter().find(|m| m.slug == DEFAULT_MODEL).or(models.first())
    {
        selection.model = fallback.slug.clone();
        changed = true;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Model");
        match &view.models {
            ModelCatalog::Loaded(models) if !models.is_empty() => {
                let selected_text = models.iter().find(|m| m.slug == selection.model).map_or(selection.model.clone(), |m| m.display_name.clone());
                egui::ComboBox::from_id_salt("onboarding-chatgpt-model").selected_text(selected_text).width(widgets::field_width(ui, 260.0, 0.0)).truncate().show_ui(ui, |ui| {
                    for model in models {
                        changed |= ui.selectable_value(&mut selection.model, model.slug.clone(), &model.display_name).changed();
                    }
                });
            }
            ModelCatalog::Failed(_) => {
                ui.label(RichText::new(&selection.model).strong());
                if ui.small_button("Load the list again").clicked() {
                    runtime.chatgpt_load_models(&name);
                }
            }
            _ => {
                ui.label(RichText::new(&selection.model).strong());
                ui.spinner();
            }
        }
    });
    if let ModelCatalog::Failed(error) = &view.models {
        ui.add(egui::Label::new(RichText::new(format!("Could not load the models: {error}")).small().color(palette.danger)).wrap());
    }
    let ready = !selection.model.trim().is_empty();
    if changed {
        app.mark_dirty();
    }

    // The notice ChatGPT asks apps to show once a plan is connected; Continue acknowledges it.
    ui.add_space(4.0);
    theme::banner(ui, palette.success).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new("You're using your ChatGPT plan").strong());
        ui.label("Eligible usage in this app uses your ChatGPT plan. Manage usage in your ChatGPT settings.");
        manage_usage_link(ui, &runtime);
    });
    ready.then_some(name)
}

fn api_step(app: &mut LlmSubsApp, guide: &mut Onboarding, ui: &mut Ui) -> Action {
    let Some(form) = guide.api_form.as_mut() else { return Action::Stay };
    ui.label("Use an API key from OpenAI or Anthropic, or a server that speaks the OpenAI protocol, such as Ollama, LM Studio or OpenRouter.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (kind, label) in [(ProviderKind::OpenAi, "OpenAI"), (ProviderKind::Anthropic, "Anthropic"), (ProviderKind::OpenAiCompatible, "OpenAI-compatible")] {
            if ui.selectable_label(form.kind == kind, label).clicked() && form.kind != kind {
                *form = ApiForm::new(kind);
            }
        }
    });
    ui.add_space(4.0);
    let field_width = widgets::field_width(ui, 340.0, 0.0);
    egui::Grid::new("onboarding-api").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        if form.kind == ProviderKind::OpenAiCompatible {
            ui.label("Server URL");
            ui.add(egui::TextEdit::singleline(&mut form.base_url).desired_width(field_width));
            ui.end_row();
        }
        ui.label("API key");
        let hint = if form.kind == ProviderKind::OpenAiCompatible { "optional for local servers" } else { "sk-…" };
        ui.add(egui::TextEdit::singleline(&mut form.api_key).password(true).desired_width(field_width).hint_text(hint));
        ui.end_row();
        ui.label("Model");
        ui.add(egui::TextEdit::singleline(&mut form.model).desired_width(field_width).hint_text("model id as the provider names it"));
        ui.end_row();
    });
    widgets::hint(ui, "The key is stored in the configuration file. Prices, effort and a connection test are under Models.");
    ui.add_space(8.0);

    let complete = form.is_complete();
    let mut proceed = false;
    widgets::trailing_row(
        ui,
        |ui| proceed = ui.add_enabled(complete, egui::Button::new("Continue")).clicked(),
        |ui| {
            if ui.link("Use ChatGPT instead").clicked() {
                guide.api_form = None;
            }
        },
    );
    let Some(form) = guide.api_form.as_ref().filter(|_| proceed) else { return Action::Stay };
    guide.api_provider = Some(add_api_provider(&mut app.draft, form, guide.api_provider.as_deref()));
    app.mark_dirty();
    Action::GoTo(Step::Language)
}

/// Adds the form's provider and makes it the translator; `previous` is replaced when the
/// guide added one before. Returns the provider's name.
fn add_api_provider(draft: &mut Config, form: &ApiForm, previous: Option<&str>) -> String {
    if let Some(previous) = previous {
        draft.providers.retain(|p| p.name != previous);
    }
    let base = match form.kind {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAiCompatible | ProviderKind::ChatGpt => "local",
    };
    let name = unique_provider_name(draft, base);
    draft.providers.push(ProviderProfile {
        name: name.clone(),
        kind: form.kind,
        base_url: form.base_url.trim().to_string(),
        api_key: form.api_key.trim().to_string(),
        ..ProviderProfile::default()
    });
    draft.models.translation = ModelSelection { provider: name.clone(), model: form.model.trim().to_string(), ..ModelSelection::default() };
    name
}

fn unique_provider_name(draft: &Config, base: &str) -> String {
    let mut name = base.to_string();
    let mut counter = 2;
    while draft.provider(&name).is_some() {
        name = format!("{base}-{counter}");
        counter += 1;
    }
    name
}

fn language_step(app: &mut LlmSubsApp, ui: &mut Ui) -> Action {
    step_caption(ui, 2, "Choose your language");
    ui.label("Which language should subtitles be translated into?");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Translate into");
        let mut code = app.draft.general.target_language.clone();
        if widgets::language_picker(ui, "onboarding-language", &mut code, &mut app.language_filter) {
            app.draft.general.target_language = code;
            app.mark_dirty();
        }
    });
    widgets::hint(ui, "Change it any time under Translation, along with where the subtitles are saved.");
    ui.add_space(8.0);

    let chosen = !app.draft.general.target_language.trim().is_empty();
    let (mut finish, mut back) = (false, false);
    widgets::trailing_row(
        ui,
        |ui| finish = ui.add_enabled(chosen, egui::Button::new("Finish")).clicked(),
        |ui| back = ui.button("Back").clicked(),
    );
    if finish {
        app.toast("All set. Drop media on the window or add a watched folder to start translating.");
        Action::Close
    } else if back {
        Action::GoTo(Step::Provider)
    } else {
        Action::Stay
    }
}

fn ffmpeg_status(app: &LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let runtime = app.runtime().clone();
    match runtime.download_status() {
        DownloadStatus::Running(progress) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("FFmpeg").small().color(palette.muted));
                let width = ui.available_width();
                download_progress_bar(ui, &progress, width);
            });
            widgets::hint(ui, "LLM Subs needs FFmpeg to read media files, so it is fetched in the background.");
        }
        DownloadStatus::Failed(error) => {
            ui.horizontal(|ui| {
                widgets::status_dot(ui, palette.danger);
                ui.label(RichText::new("FFmpeg could not be downloaded").small());
                if ui.small_button("Retry").clicked() {
                    runtime.start_ffmpeg_download();
                }
            });
            ui.add(egui::Label::new(RichText::new(error).small().color(palette.danger)).wrap());
            widgets::hint(ui, "You can also point LLM Subs at an installed copy under Setup.");
        }
        DownloadStatus::Idle | DownloadStatus::Finished => ui.horizontal(|ui| match runtime.ffmpeg_status() {
            FfmpegStatus::Available { .. } => {
                widgets::status_dot(ui, palette.success);
                ui.label(RichText::new("FFmpeg is ready").small().color(palette.muted));
            }
            FfmpegStatus::Missing => {
                widgets::status_dot(ui, palette.danger);
                ui.label(RichText::new("FFmpeg not found").small().color(palette.muted));
                if ui.small_button("Download").clicked() {
                    runtime.start_ffmpeg_download();
                }
            }
        })
        .inner,
    }
}
