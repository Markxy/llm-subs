use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Ui};

use crate::app::{AppRuntime, ChatGptSignIn, ProviderTestState};
use crate::config::{ModelPricing, ModelSelection, ProviderKind, ProviderProfile};
use crate::llm::chatgpt::{ModelCatalog, SignInProgress, DEFAULT_MODEL};
use crate::llm::{builtin_price, format_token_count, format_usd};
use crate::ui::{theme, widgets, LlmSubsApp};

pub fn show(app: &mut LlmSubsApp, ui: &mut Ui) {
    widgets::page_title(ui, "Models", "Which model translates, how LLM Subs reaches it, and what it costs.");

    widgets::section_title(ui, "Translation model");
    theme::card(ui).show(ui, |ui| show_translation_model(app, ui));

    ui.add_space(16.0);
    widgets::section_title(ui, "Providers");
    widgets::hint(
        ui,
        "Each provider is one service: a ChatGPT sign-in, an API key, or a local server such as Ollama, LM Studio or llama.cpp. \
The translation model above uses one of them.",
    );
    ui.add_space(4.0);
    show_providers(app, ui);
}

fn show_translation_model(app: &mut LlmSubsApp, ui: &mut Ui) {
    let runtime = app.runtime().clone();
    let providers = app.draft.providers.clone();
    let selection = &mut app.draft.models.translation;
    let mut changed = false;
    let current = providers.iter().find(|p| p.name == selection.provider);
    let selected_label = current.map(provider_label).unwrap_or_else(|| "Select…".to_string());
    let was_chatgpt = current.is_some_and(|p| p.kind == ProviderKind::ChatGpt);
    widgets::form_row(ui, "Provider", |ui| {
        egui::ComboBox::from_id_salt("translation-provider")
            .selected_text(selected_label)
            .width(widgets::field_width(ui, 300.0, 0.0))
            .truncate()
            .show_ui(ui, |ui| {
                for provider in &providers {
                    if ui.selectable_value(&mut selection.provider, provider.name.clone(), provider_label(provider)).changed() {
                        changed = true;
                        // Another kind's model id means nothing to ChatGPT.
                        if provider.kind == ProviderKind::ChatGpt && !was_chatgpt {
                            selection.model = DEFAULT_MODEL.to_string();
                        }
                        let accepts_effort = selection.effort.as_deref().is_some_and(|effort| provider.kind.effort_levels().contains(&effort));
                        if !accepts_effort {
                            selection.effort = None;
                        }
                    }
                }
            });
    });
    match providers.iter().find(|p| p.name == selection.provider) {
        None => widgets::hint(ui, "Add a provider below, then choose it here."),
        Some(provider) if provider.kind == ProviderKind::ChatGpt => {
            changed |= chatgpt_model_rows(ui, &runtime, provider, selection, &mut app.requested_model_lists);
        }
        Some(provider) => changed |= api_model_rows(ui, provider.kind, selection),
    }
    if changed {
        app.mark_dirty();
    }
}

fn provider_label(provider: &ProviderProfile) -> String {
    format!("{}  ·  {}", provider.name, provider.kind.label())
}

fn chatgpt_model_rows(
    ui: &mut Ui,
    runtime: &Arc<AppRuntime>,
    provider: &ProviderProfile,
    selection: &mut ModelSelection,
    requested_model_lists: &mut HashSet<String>,
) -> bool {
    let palette = theme::palette(ui.ctx());
    let view = runtime.llm().chatgpt.view(&provider.name);
    let Some(account) = &view.account else {
        widgets::hint(ui, "Sign this provider in to ChatGPT below, then choose a model here.");
        return false;
    };
    if account.usage_sharing && view.models == ModelCatalog::NotLoaded && requested_model_lists.insert(provider.name.clone()) {
        runtime.chatgpt_load_models(&provider.name);
    }
    let models = match &view.models {
        ModelCatalog::Loaded(models) => models.as_slice(),
        _ => &[],
    };
    let mut changed = false;
    if selection.model.trim().is_empty() {
        selection.model = DEFAULT_MODEL.to_string();
        changed = true;
    }
    widgets::form_row(ui, "Model", |ui| {
        if models.is_empty() {
            changed |= ui.add(egui::TextEdit::singleline(&mut selection.model).desired_width(widgets::field_width(ui, 300.0, 100.0)).hint_text("model slug")).changed();
        } else {
            let selected_text = models
                .iter()
                .find(|m| m.slug == selection.model)
                .map(|m| m.display_name.clone())
                .unwrap_or_else(|| if selection.model.is_empty() { "Select…".to_string() } else { selection.model.clone() });
            let width = widgets::field_width(ui, 300.0, 100.0);
            egui::ComboBox::from_id_salt("translation-chatgpt-model").selected_text(selected_text).width(width).truncate().show_ui(ui, |ui| {
                for model in models {
                    changed |= ui.selectable_value(&mut selection.model, model.slug.clone(), &model.display_name).changed();
                }
            });
        }
        if view.models == ModelCatalog::Loading {
            ui.spinner();
        } else if ui.small_button("Refresh list").clicked() {
            runtime.chatgpt_load_models(&provider.name);
        }
    });
    if let ModelCatalog::Failed(error) = &view.models {
        ui.label(RichText::new(format!("Could not load the models: {error}")).small().color(palette.danger));
    }

    let chosen = models.iter().find(|m| m.slug == selection.model);
    let levels: Vec<String> = match chosen {
        Some(model) if !model.reasoning_levels.is_empty() => model.reasoning_levels.clone(),
        _ => ProviderKind::ChatGpt.effort_levels().iter().map(|level| level.to_string()).collect(),
    };
    let default_label = match chosen.and_then(|m| m.default_reasoning_level.as_deref()) {
        Some(level) => format!("model default ({level})"),
        None => "model default".to_string(),
    };
    widgets::form_row(ui, "Effort", |ui| {
        changed |= effort_picker(ui, "translation-effort", &mut selection.effort, &levels, &default_label);
    });
    if let Some(window) = chosen.and_then(|m| m.context_window) {
        widgets::hint(ui, &format!("Context window: {} tokens", format_token_count(window)));
    }
    ui.add_space(2.0);
    using_plan_line(ui, runtime);
    changed
}

fn api_model_rows(ui: &mut Ui, kind: ProviderKind, selection: &mut ModelSelection) -> bool {
    let mut changed = false;
    let model_hint = match kind {
        ProviderKind::Anthropic => "e.g. claude-opus-5-5",
        ProviderKind::OpenAi => "e.g. gpt-5",
        _ => "model id as the provider names it",
    };
    widgets::form_row(ui, "Model", |ui| {
        changed |= ui.add(egui::TextEdit::singleline(&mut selection.model).desired_width(widgets::field_width(ui, 300.0, 0.0)).hint_text(model_hint)).changed();
    });
    if !kind.effort_levels().is_empty() {
        let levels: Vec<String> = kind.effort_levels().iter().map(|level| level.to_string()).collect();
        widgets::form_row(ui, "Effort", |ui| {
            changed |= effort_picker(ui, "translation-effort", &mut selection.effort, &levels, "model default");
        });
    }
    widgets::form_row(ui, "Max output tokens", |ui| {
        changed |= widgets::optional_u32(ui, &mut selection.max_output_tokens, "provider default");
    });
    widgets::form_row(ui, "Temperature", |ui| {
        changed |= widgets::optional_f32(ui, &mut selection.temperature, "not sent");
    });
    widgets::form_row(ui, "Price per million tokens", |ui| {
        changed |= price_editor(ui, kind, selection);
    });
    changed
}

fn effort_picker(ui: &mut Ui, id: &str, effort: &mut Option<String>, levels: &[String], default_label: &str) -> bool {
    let mut changed = false;
    let current = effort.clone().unwrap_or_default();
    egui::ComboBox::from_id_salt(id).selected_text(if current.is_empty() { default_label } else { current.as_str() }).show_ui(ui, |ui| {
        if ui.selectable_label(current.is_empty(), default_label).clicked() {
            *effort = None;
            changed = true;
        }
        for level in levels {
            if ui.selectable_label(current == *level, level).clicked() {
                *effort = Some(level.clone());
                changed = true;
            }
        }
    });
    changed
}

fn price_editor(ui: &mut Ui, kind: ProviderKind, selection: &mut ModelSelection) -> bool {
    let palette = theme::palette(ui.ctx());
    let builtin = builtin_price(kind, &selection.model);
    let mut changed = false;
    ui.vertical(|ui| {
        match selection.pricing.as_mut() {
            Some(pricing) => {
                ui.horizontal(|ui| {
                    let mut cached = pricing.cached_input_per_million.unwrap_or(pricing.input_per_million);
                    ui.label("input");
                    changed |= ui.add(price_value(&mut pricing.input_per_million)).changed();
                    ui.label("cached input");
                    if ui.add(price_value(&mut cached)).changed() {
                        pricing.cached_input_per_million = Some(cached);
                        changed = true;
                    }
                    ui.label("output");
                    changed |= ui.add(price_value(&mut pricing.output_per_million)).changed();
                });
            }
            None => match builtin {
                Some(price) => {
                    ui.label(RichText::new(format!("{} (list price)", describe_price(&price))).color(palette.muted));
                }
                None => {
                    ui.label(RichText::new("unknown, so costs are not calculated").color(palette.muted));
                }
            },
        }
        let mut custom = selection.pricing.is_some();
        if ui.checkbox(&mut custom, "Use my own price").changed() {
            selection.pricing = custom.then(|| builtin.unwrap_or_default());
            changed = true;
        }
    });
    changed
}

fn price_value(value: &mut f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value).prefix("$").speed(0.05).range(0.0..=1000.0).max_decimals(3)
}

fn describe_price(price: &ModelPricing) -> String {
    let cached = price.cached_input_per_million.unwrap_or(price.input_per_million);
    format!(
        "{} input · {} cached · {} output",
        format_usd(price.input_per_million),
        format_usd(cached),
        format_usd(price.output_per_million)
    )
}

/// The indicator ChatGPT asks apps to show wherever its plan is in use.
fn using_plan_line(ui: &mut Ui, runtime: &Arc<AppRuntime>) {
    let palette = theme::palette(ui.ctx());
    ui.horizontal(|ui| {
        widgets::status_dot(ui, palette.success);
        ui.label(RichText::new("Using ChatGPT plan").color(palette.muted));
        manage_usage_link(ui, runtime);
    });
}

pub(crate) fn manage_usage_link(ui: &mut Ui, runtime: &Arc<AppRuntime>) {
    if ui.link("Manage usage").clicked()
        && let Err(error) = runtime.open_chatgpt_usage_page()
    {
        tracing::warn!(error = %format!("{error:#}"), "could not open the usage page");
    }
}

fn show_providers(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    let runtime = app.runtime().clone();
    let mut remove_index = None;
    let translation_model = app.draft.models.translation.clone();
    let show_keys = app.show_api_keys;

    for index in 0..app.draft.providers.len() {
        let mut changed = false;
        let mut test_request: Option<(ProviderProfile, String, Option<String>)> = None;
        let mut renamed_from = None;
        theme::card(ui).show(ui, |ui| {
            let provider = &mut app.draft.providers[index];
            widgets::trailing_row(
                ui,
                |ui| {
                    let in_use = translation_model.provider == provider.name;
                    if ui
                        .add_enabled(!in_use, egui::Button::new("Remove").small())
                        .on_disabled_hover_text("This provider translates; choose another one above first.")
                        .clicked()
                    {
                        remove_index = Some(index);
                    }
                    widgets::status_chip(ui, provider.kind.label(), palette.muted);
                },
                |ui| widgets::truncated(ui, RichText::new(if provider.name.is_empty() { "New provider" } else { provider.name.as_str() }).strong().size(15.0)),
            );
            widgets::form_row(ui, "Name", |ui| {
                let previous_name = provider.name.clone();
                if ui.add(egui::TextEdit::singleline(&mut provider.name).desired_width(widgets::field_width(ui, 220.0, 0.0))).changed() {
                    renamed_from = Some(previous_name);
                    changed = true;
                }
            });
            if provider.kind == ProviderKind::ChatGpt {
                ui.add_space(4.0);
                chatgpt_account_panel(ui, &runtime, provider);
            } else {
                widgets::form_row(ui, "Base URL", |ui| {
                    changed |= ui.add(egui::TextEdit::singleline(&mut provider.base_url).desired_width(widgets::field_width(ui, 420.0, 0.0))).changed();
                });
                widgets::form_row(ui, "API key", |ui| {
                    let hint = if provider.kind == ProviderKind::OpenAiCompatible { "optional for local servers" } else { "sk-…" };
                    let width = widgets::field_width(ui, 420.0, 0.0);
                    changed |= ui.add(egui::TextEdit::singleline(&mut provider.api_key).password(!show_keys).desired_width(width).hint_text(hint)).changed();
                });
                widgets::form_row(ui, "…or from environment variable", |ui| {
                    let mut env_name = provider.api_key_env.clone().unwrap_or_default();
                    if ui.add(egui::TextEdit::singleline(&mut env_name).desired_width(widgets::field_width(ui, 220.0, 0.0)).hint_text("OPENAI_API_KEY")).changed() {
                        provider.api_key_env = (!env_name.trim().is_empty()).then(|| env_name.trim().to_string());
                        changed = true;
                    }
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let id = ui.id().with(("test-model", &provider.name));
                let is_translator = translation_model.provider == provider.name;
                let default_model = if is_translator { translation_model.model.clone() } else { String::new() };
                let mut test_model = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or(default_model);
                ui.add(egui::TextEdit::singleline(&mut test_model).desired_width(widgets::field_width(ui, 220.0, 140.0)).hint_text("model to test with"));
                ui.data_mut(|d| d.insert_temp(id, test_model.clone()));
                if ui.button("Test connection").clicked() {
                    let effort = if is_translator && test_model == translation_model.model { translation_model.effort.clone() } else { None };
                    test_request = Some((provider.clone(), test_model, effort));
                }
                match app.provider_tests.get(&provider.name).map(|t| t.lock().clone()) {
                    Some(ProviderTestState::Running) => {
                        ui.spinner();
                        ui.label(RichText::new("Testing…").color(palette.muted));
                    }
                    Some(ProviderTestState::Succeeded(message)) => {
                        ui.label(RichText::new("✔").color(palette.success));
                        ui.add(egui::Label::new(RichText::new(message).small().color(palette.success)).wrap());
                    }
                    Some(ProviderTestState::Failed(message)) => {
                        ui.label(RichText::new("✖").color(palette.danger));
                        ui.add(egui::Label::new(RichText::new(message).small().color(palette.danger)).wrap());
                    }
                    None => {}
                }
            });
        });
        if let Some((provider, model, effort)) = test_request {
            if model.trim().is_empty() {
                app.toast("Enter a model name to test with");
            } else {
                let test = runtime.test_provider(provider.clone(), model, effort);
                app.provider_tests.insert(provider.name, test);
            }
        }
        if let Some(previous_name) = renamed_from
            && app.draft.models.translation.provider == previous_name
        {
            app.draft.models.translation.provider = app.draft.providers[index].name.clone();
        }
        if changed {
            app.mark_dirty();
        }
        ui.add_space(6.0);
    }
    if let Some(index) = remove_index {
        app.draft.providers.remove(index);
        app.mark_dirty();
    }

    ui.horizontal(|ui| {
        ui.menu_button("Add provider", |ui| {
            let choices = [
                ("ChatGPT account (sign in with ChatGPT)", ProviderKind::ChatGpt, "chatgpt"),
                ("OpenAI API", ProviderKind::OpenAi, "openai"),
                ("Anthropic API", ProviderKind::Anthropic, "anthropic"),
                ("OpenAI-compatible (Ollama, LM Studio, OpenRouter, …)", ProviderKind::OpenAiCompatible, "local"),
            ];
            for (label, kind, name) in choices {
                if ui.button(label).clicked() {
                    let mut unique = name.to_string();
                    let mut counter = 2;
                    while app.draft.providers.iter().any(|p| p.name == unique) {
                        unique = format!("{name}-{counter}");
                        counter += 1;
                    }
                    app.draft.providers.push(ProviderProfile {
                        name: unique,
                        kind,
                        base_url: kind.default_base_url().to_string(),
                        ..ProviderProfile::default()
                    });
                    app.mark_dirty();
                    ui.close();
                }
            }
        });
        ui.checkbox(&mut app.show_api_keys, "Show keys");
    });
}

fn chatgpt_account_panel(ui: &mut Ui, runtime: &Arc<AppRuntime>, provider: &ProviderProfile) {
    let palette = theme::palette(ui.ctx());
    let view = runtime.llm().chatgpt.view(&provider.name);
    if let SignInProgress::Waiting { authorize_url } = &view.sign_in {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Finish signing in in your browser…");
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
                runtime.chatgpt_cancel_sign_in(&provider.name);
            }
        });
        return;
    }
    match &view.account {
        Some(account) => {
            widgets::trailing_row(
                ui,
                |ui| {
                    if ui.small_button("Sign out").clicked() {
                        runtime.chatgpt_sign_out(&provider.name);
                    }
                    manage_usage_link(ui, runtime);
                },
                |ui| {
                    widgets::status_dot(ui, if account.usage_sharing { palette.success } else { palette.warning });
                    widgets::truncated(ui, RichText::new(format!("Signed in as {}", account.display_name())).strong());
                },
            );
            if !account.usage_sharing {
                theme::banner(ui, palette.warning).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Usage sharing is turned off, so LLM Subs cannot translate with this plan.");
                        if ui.button("Enable usage sharing").clicked() {
                            runtime.chatgpt_sign_in(&provider.name, ChatGptSignIn::EnableUsageSharing);
                        }
                    });
                });
            }
        }
        None => {
            if let SignInProgress::Failed(error) = &view.sign_in {
                ui.label(RichText::new(error).small().color(palette.danger));
            }
            ui.horizontal(|ui| {
                if chatgpt_button(ui).clicked() {
                    runtime.chatgpt_sign_in(&provider.name, ChatGptSignIn::Connect);
                }
                if view.registered
                    && ui
                        .small_button("Use a different account")
                        .on_hover_text("Forget the ChatGPT account connected before, so another one can be used")
                        .clicked()
                {
                    runtime.chatgpt_sign_in(&provider.name, ChatGptSignIn::SwitchAccount);
                }
            });
            widgets::hint(
                ui,
                "Uses your ChatGPT Plus or Pro plan instead of an API key; translations count toward the plan's usage limits. \
Sign in with ChatGPT is offered for personal and open-source use.",
            );
        }
    }
}

/// The sign-in button in ChatGPT's approved black-or-white style.
pub(crate) fn chatgpt_button(ui: &mut Ui) -> egui::Response {
    let (fill, text) = if ui.visuals().dark_mode { (Color32::WHITE, Color32::BLACK) } else { (Color32::BLACK, Color32::WHITE) };
    ui.add(
        egui::Button::new(RichText::new("Continue with ChatGPT").color(text).strong())
            .fill(fill)
            .corner_radius(egui::CornerRadius::same(16))
            .min_size(egui::vec2(0.0, 32.0)),
    )
}
