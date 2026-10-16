//! A running tally of model usage per day, provider and model that survives restarts
//! and cleared job history. Every finished request is recorded, also those of jobs
//! that later failed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config::store::write_atomically;
use crate::config::ProviderKind;

use super::pricing::Cost;
use super::types::TokenUsage;

const FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub date: NaiveDate,
    pub provider: String,
    pub provider_kind: ProviderKind,
    pub model: String,
    pub requests: u64,
    pub usage: TokenUsage,
    /// Dollars spent on the requests whose price is known.
    #[serde(default)]
    pub cost_usd: f64,
    /// Requests billed per token at a price LLM Subs does not know.
    #[serde(default)]
    pub unpriced_requests: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LedgerFile {
    version: u32,
    records: Vec<UsageRecord>,
}

/// Usage of one provider and model over the summarised period.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub provider: String,
    pub provider_kind: ProviderKind,
    pub model: String,
    pub requests: u64,
    pub usage: TokenUsage,
    pub cost_usd: f64,
    pub unpriced_requests: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageSummary {
    pub requests: u64,
    pub usage: TokenUsage,
    pub cost_usd: f64,
    pub unpriced_requests: u64,
    /// Requests that ran on a ChatGPT plan rather than billed per token.
    pub plan_requests: u64,
    pub models: Vec<ModelUsage>,
}

struct Inner {
    path: Option<PathBuf>,
    records: Mutex<Vec<UsageRecord>>,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[derive(Clone)]
pub struct UsageLedger {
    inner: Arc<Inner>,
}

impl UsageLedger {
    pub fn open(path: &Path) -> Self {
        let records = read_records(path).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the usage statistics; starting a new tally");
            Vec::new()
        });
        Self::with_records(Some(path.to_path_buf()), records)
    }

    /// A ledger that is not saved anywhere.
    pub fn in_memory() -> Self {
        Self::with_records(None, Vec::new())
    }

    fn with_records(path: Option<PathBuf>, records: Vec<UsageRecord>) -> Self {
        Self { inner: Arc::new(Inner { path, records: Mutex::new(records), on_change: Mutex::new(None) }) }
    }

    pub fn set_change_listener(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.inner.on_change.lock() = Some(callback);
    }

    pub fn record(&self, provider: &str, provider_kind: ProviderKind, model: &str, usage: &TokenUsage, cost: Option<Cost>) {
        let date = chrono::Local::now().date_naive();
        {
            let mut records = self.inner.records.lock();
            // Another LLM Subs process may have recorded usage since this one last looked.
            if let Some(path) = &self.inner.path
                && let Ok(on_disk) = read_records(path)
            {
                *records = on_disk;
            }
            let index = match records.iter().position(|r| r.date == date && r.provider == provider && r.model == model) {
                Some(index) => index,
                None => {
                    records.push(UsageRecord {
                        date,
                        provider: provider.to_string(),
                        provider_kind,
                        model: model.to_string(),
                        requests: 0,
                        usage: TokenUsage::default(),
                        cost_usd: 0.0,
                        unpriced_requests: 0,
                    });
                    records.len() - 1
                }
            };
            let record = &mut records[index];
            record.provider_kind = provider_kind;
            record.requests += 1;
            record.usage += *usage;
            match cost {
                Some(Cost::Usd(amount)) => record.cost_usd += amount,
                Some(Cost::ChatGptPlan) => {}
                None => record.unpriced_requests += 1,
            }
            if let Some(path) = &self.inner.path
                && let Err(error) = write_records(path, &records)
            {
                tracing::warn!(%error, "could not save the usage statistics");
            }
        }
        self.notify();
    }

    /// Totals from `since` (inclusive) on, or over everything recorded.
    pub fn summary(&self, since: Option<NaiveDate>) -> UsageSummary {
        let records = self.inner.records.lock();
        let mut summary = UsageSummary::default();
        for record in records.iter().filter(|r| since.is_none_or(|since| r.date >= since)) {
            summary.requests += record.requests;
            summary.usage += record.usage;
            summary.cost_usd += record.cost_usd;
            summary.unpriced_requests += record.unpriced_requests;
            if record.provider_kind == ProviderKind::ChatGpt {
                summary.plan_requests += record.requests;
            }
            match summary.models.iter_mut().find(|m| m.provider == record.provider && m.model == record.model) {
                Some(row) => {
                    row.requests += record.requests;
                    row.usage += record.usage;
                    row.cost_usd += record.cost_usd;
                    row.unpriced_requests += record.unpriced_requests;
                }
                None => summary.models.push(ModelUsage {
                    provider: record.provider.clone(),
                    provider_kind: record.provider_kind,
                    model: record.model.clone(),
                    requests: record.requests,
                    usage: record.usage,
                    cost_usd: record.cost_usd,
                    unpriced_requests: record.unpriced_requests,
                }),
            }
        }
        summary.models.sort_by_key(|row| std::cmp::Reverse(row.usage.total()));
        summary
    }

    pub fn clear(&self) -> Result<()> {
        let mut records = self.inner.records.lock();
        records.clear();
        if let Some(path) = &self.inner.path {
            write_records(path, &records)?;
        }
        drop(records);
        self.notify();
        Ok(())
    }

    fn notify(&self) {
        let callback = self.inner.on_change.lock().clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

fn read_records(path: &Path) -> Result<Vec<UsageRecord>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<LedgerFile>(&bytes).with_context(|| format!("parsing {}", path.display()))?.records),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_records(path: &Path, records: &[UsageRecord]) -> Result<()> {
    let file = LedgerFile { version: FILE_VERSION, records: records.to_vec() };
    write_atomically(path, &serde_json::to_vec_pretty(&file)?).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tallies_per_model_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let ledger = UsageLedger::open(&path);
        let usage = TokenUsage { input_tokens: 100, output_tokens: 50, ..Default::default() };
        ledger.record("anthropic", ProviderKind::Anthropic, "claude-opus-5-5", &usage, Some(Cost::Usd(0.5)));
        ledger.record("anthropic", ProviderKind::Anthropic, "claude-opus-5-5", &usage, Some(Cost::Usd(0.25)));
        ledger.record("chatgpt", ProviderKind::ChatGpt, "gpt-6.1-sol", &usage, Some(Cost::ChatGptPlan));
        ledger.record("local", ProviderKind::OpenAiCompatible, "llama", &usage, None);

        let summary = UsageLedger::open(&path).summary(None);
        assert_eq!(summary.requests, 4);
        assert_eq!(summary.usage.input_tokens, 400);
        assert!((summary.cost_usd - 0.75).abs() < 1e-9);
        assert_eq!(summary.plan_requests, 1);
        assert_eq!(summary.unpriced_requests, 1);
        assert_eq!(summary.models.len(), 3);
        assert_eq!(summary.models[0].requests, 2);

        let tomorrow = chrono::Local::now().date_naive().succ_opt().unwrap();
        assert_eq!(ledger.summary(Some(tomorrow)).requests, 0);
        ledger.clear().unwrap();
        assert_eq!(UsageLedger::open(&path).summary(None).requests, 0);
    }
}
