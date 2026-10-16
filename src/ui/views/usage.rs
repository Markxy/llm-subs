use chrono::NaiveDate;
use eframe::egui::{self, RichText, Ui};

use crate::config::ProviderKind;
use crate::llm::{format_token_count, format_usd, ModelUsage, UsageSummary};
use crate::ui::{theme, widgets, LlmSubsApp};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UsagePeriod {
    Today,
    Week,
    #[default]
    Month,
    AllTime,
}

impl UsagePeriod {
    const ALL: [UsagePeriod; 4] = [UsagePeriod::Today, UsagePeriod::Week, UsagePeriod::Month, UsagePeriod::AllTime];

    fn label(self) -> &'static str {
        match self {
            UsagePeriod::Today => "Today",
            UsagePeriod::Week => "Last 7 days",
            UsagePeriod::Month => "Last 30 days",
            UsagePeriod::AllTime => "All time",
        }
    }

    fn since(self) -> Option<NaiveDate> {
        let today = chrono::Local::now().date_naive();
        match self {
            UsagePeriod::Today => Some(today),
            UsagePeriod::Week => Some(today - chrono::Days::new(6)),
            UsagePeriod::Month => Some(today - chrono::Days::new(29)),
            UsagePeriod::AllTime => None,
        }
    }
}

pub fn show(app: &mut LlmSubsApp, ui: &mut Ui) {
    let palette = theme::palette(ui.ctx());
    widgets::page_title(ui, "Usage", "Tokens and cost of every model request, including requests of jobs that failed later.");

    ui.horizontal(|ui| {
        for period in UsagePeriod::ALL {
            if ui.selectable_label(app.usage_period == period, period.label()).clicked() {
                app.usage_period = period;
            }
        }
    });
    ui.add_space(8.0);

    let summary = app.runtime().llm().usage.summary(app.usage_period.since());
    show_totals(ui, &summary);
    ui.add_space(14.0);

    widgets::section_title(ui, "By model");
    theme::card(ui).show(ui, |ui| {
        if summary.models.is_empty() {
            ui.label(RichText::new("No requests in this period.").color(palette.muted));
        } else {
            show_model_table(ui, &summary.models);
        }
    });

    let uses_chatgpt = app.draft.providers.iter().any(|p| p.kind == ProviderKind::ChatGpt) || summary.plan_requests > 0;
    if uses_chatgpt {
        ui.add_space(14.0);
        widgets::section_title(ui, "ChatGPT plan");
        theme::card(ui).show(ui, |ui| {
            ui.label(
                "Requests through a ChatGPT sign-in are not billed per token. They count toward your plan's usage limits, \
which only ChatGPT can show; on Plus the five-hour limit is shared with every other app you connected.",
            );
            if ui.link("Manage usage in ChatGPT").clicked()
                && let Err(error) = app.runtime().open_chatgpt_usage_page()
            {
                app.toast(format!("{error:#}"));
            }
        });
    }

    ui.add_space(14.0);
    ui.horizontal(|ui| {
        widgets::hint(ui, "Costs use list prices, or the price you set for a model under Models.");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("Reset…", |ui| {
                if ui.button("Delete all usage statistics").clicked() {
                    if let Err(error) = app.runtime().llm().usage.clear() {
                        app.toast(format!("Could not reset the statistics: {error:#}"));
                    }
                    ui.close();
                }
            });
        });
    });
}

fn show_totals(ui: &mut Ui, summary: &UsageSummary) {
    let usage = &summary.usage;
    let mut cost_details = Vec::new();
    if summary.plan_requests > 0 {
        cost_details.push(format!("+ {} on ChatGPT plan", plural(summary.plan_requests, "request")));
    }
    if summary.unpriced_requests > 0 {
        cost_details.push(format!("+ {} unpriced", plural(summary.unpriced_requests, "request")));
    }
    ui.columns(4, |columns| {
        widgets::stat_tile(&mut columns[0], "Requests", &summary.requests.to_string(), "");
        widgets::stat_tile(
            &mut columns[1],
            "Input tokens",
            &format_token_count(usage.input_tokens),
            &if usage.cached_input_tokens > 0 { format!("{} from cache", format_token_count(usage.cached_input_tokens)) } else { String::new() },
        );
        widgets::stat_tile(
            &mut columns[2],
            "Output tokens",
            &format_token_count(usage.output_tokens),
            &if usage.reasoning_tokens > 0 { format!("{} reasoning", format_token_count(usage.reasoning_tokens)) } else { String::new() },
        );
        widgets::stat_tile(&mut columns[3], "Cost", &format_usd(summary.cost_usd), &cost_details.join("\n"));
    });
}

fn show_model_table(ui: &mut Ui, models: &[ModelUsage]) {
    let palette = theme::palette(ui.ctx());
    egui::Grid::new("usage-by-model").num_columns(6).spacing([22.0, 8.0]).striped(true).show(ui, |ui| {
        for heading in ["Model", "Provider", "Requests", "Input", "Output", "Cost"] {
            ui.label(RichText::new(heading).small().strong().color(palette.muted));
        }
        ui.end_row();
        for row in models {
            ui.label(RichText::new(&row.model).strong());
            ui.label(&row.provider);
            ui.label(row.requests.to_string());
            ui.label(format_token_count(row.usage.input_tokens));
            ui.label(format_token_count(row.usage.output_tokens));
            let cost = if row.provider_kind == ProviderKind::ChatGpt {
                "ChatGPT plan".to_string()
            } else if row.unpriced_requests == row.requests {
                "price unknown".to_string()
            } else if row.unpriced_requests > 0 {
                format!("{} + {} unpriced", format_usd(row.cost_usd), row.unpriced_requests)
            } else {
                format_usd(row.cost_usd)
            };
            ui.label(cost);
            ui.end_row();
        }
    });
}

fn plural(count: u64, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}
