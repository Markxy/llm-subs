use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::TranslationConfig;
use crate::llm::{complete_with_retries, ChatMessage, FinishReason, LlmError, ModelHandle, StreamListener, TokenUsage};

use super::{batch_message, make_batches, parse_numbered_lines, ContinuityLine, TranslationUnit};

/// Requests in a row that may come back without a single usable line before the
/// remaining lines of a batch are given up.
const MAX_FRUITLESS_ROUNDS: usize = 3;

#[derive(Debug, Clone, Copy, Default)]
pub struct TranslationProgress {
    /// Lines translated so far, including lines still streaming in.
    pub units_done: usize,
    pub units_total: usize,
    pub requests: usize,
}

pub type ProgressCallback = Arc<dyn Fn(TranslationProgress) + Send + Sync>;

#[derive(Debug, Default)]
pub struct TranslationOutcome {
    pub translations: HashMap<usize, String>,
    pub usage: TokenUsage,
    pub requests: usize,
    pub failed_unit_ids: Vec<usize>,
}

pub struct Translator {
    model: ModelHandle,
    config: TranslationConfig,
    system_prompt: String,
}

#[derive(Default)]
struct SharedState {
    translations: HashMap<usize, String>,
    usage: TokenUsage,
    requests: usize,
    units_total: usize,
    /// Lines received so far by each request that is still streaming.
    streaming_lines: HashMap<usize, usize>,
    next_request_id: usize,
}

impl SharedState {
    fn progress(&self) -> TranslationProgress {
        let streaming: usize = self.streaming_lines.values().sum();
        TranslationProgress {
            units_done: (self.translations.len() + streaming).min(self.units_total),
            units_total: self.units_total,
            requests: self.requests,
        }
    }
}

/// Counts finished answer lines while a response streams in.
struct LineCounter {
    request_id: usize,
    limit: usize,
    state: Arc<Mutex<SharedState>>,
    progress: ProgressCallback,
}

impl LineCounter {
    fn set_lines(&self, update: impl FnOnce(usize) -> usize) {
        let snapshot = {
            let mut state = self.state.lock();
            let lines = state.streaming_lines.entry(self.request_id).or_default();
            let updated = update(*lines).min(self.limit);
            if updated == *lines {
                return;
            }
            *lines = updated;
            state.progress()
        };
        (self.progress)(snapshot);
    }
}

impl StreamListener for LineCounter {
    fn text(&self, delta: &str) {
        let new_lines = delta.matches('\n').count();
        if new_lines > 0 {
            self.set_lines(|lines| lines + new_lines);
        }
    }

    fn restart(&self) {
        self.set_lines(|_| 0);
    }
}

impl Translator {
    pub fn new(model: ModelHandle, config: TranslationConfig, system_prompt: String) -> Self {
        Self { model, config, system_prompt }
    }

    pub async fn translate(
        &self,
        units: &[TranslationUnit],
        progress: ProgressCallback,
        cancel: &CancellationToken,
    ) -> Result<TranslationOutcome> {
        let batches = make_batches(units, self.config.lines_per_request, self.config.max_chars_per_request);
        let state = Arc::new(Mutex::new(SharedState { units_total: units.len(), ..Default::default() }));
        progress(state.lock().progress());

        let parallel = self.config.parallel_requests.max(1);
        let mut failed_unit_ids = Vec::new();
        let mut stream = futures::stream::iter(batches)
            .map(|batch| {
                let state = state.clone();
                let progress = progress.clone();
                async move { self.translate_batch(units, batch.unit_ids, &state, &progress, cancel).await }
            })
            .buffer_unordered(parallel);
        while let Some(result) = stream.next().await {
            failed_unit_ids.extend(result?);
        }
        drop(stream);

        let state = Arc::try_unwrap(state).map_err(|_| anyhow::anyhow!("translation state still shared")).map(Mutex::into_inner)?;
        failed_unit_ids.sort_unstable();
        Ok(TranslationOutcome { translations: state.translations, usage: state.usage, requests: state.requests, failed_unit_ids })
    }

    /// The units right before `first_id`, with translations where they exist yet.
    fn continuity_before(&self, units: &[TranslationUnit], first_id: usize, state: &Mutex<SharedState>) -> Vec<ContinuityLine> {
        let start = first_id.saturating_sub(self.config.continuity_lines);
        let guard = state.lock();
        units[start..first_id]
            .iter()
            .map(|unit| ContinuityLine { source: unit.text.clone(), translation: guard.translations.get(&unit.id).cloned() })
            .collect()
    }

    /// Translates one batch. Lines the model skipped or could not fit into its output
    /// are requested again, picking up right where the answer stopped, for as long as
    /// each request makes progress. Returns the ids that never came back.
    async fn translate_batch(
        &self,
        units: &[TranslationUnit],
        unit_ids: Vec<usize>,
        state: &Arc<Mutex<SharedState>>,
        progress: &ProgressCallback,
        cancel: &CancellationToken,
    ) -> Result<Vec<usize>> {
        let mut pending = unit_ids;
        let mut fruitless_rounds = 0;
        while !pending.is_empty() && fruitless_rounds < MAX_FRUITLESS_ROUNDS {
            let continuity = self.continuity_before(units, pending[0], state);
            let batch_units: Vec<&TranslationUnit> = pending.iter().map(|&id| &units[id]).collect();
            let mut request = self.model.request(Some(self.system_prompt.clone()), vec![ChatMessage::user(batch_message(&batch_units, &continuity))]);
            let request_id = {
                let mut guard = state.lock();
                guard.next_request_id += 1;
                guard.next_request_id
            };
            request.listener = Some(Arc::new(LineCounter { request_id, limit: pending.len(), state: state.clone(), progress: progress.clone() }));

            let result = complete_with_retries(self.model.client.as_ref(), request, self.model.max_retries, cancel).await;
            state.lock().streaming_lines.remove(&request_id);
            let response = match result {
                Ok(response) => response,
                Err(LlmError::ContextTooLong(message)) if pending.len() > 1 => {
                    tracing::warn!(lines = pending.len(), %message, "batch does not fit the model's context window, splitting it");
                    return self.translate_halves(units, pending, state, progress, cancel).await;
                }
                Err(error) => return Err(anyhow::Error::new(error).context("translation request failed")),
            };

            let parsed = parse_numbered_lines(&response.text);
            let (translated, still_missing): (Vec<usize>, Vec<usize>) = pending.iter().partition(|id| parsed.contains_key(id));
            let snapshot = {
                let mut guard = state.lock();
                guard.usage += response.usage;
                guard.requests += 1;
                for id in &translated {
                    guard.translations.insert(*id, parsed[id].clone());
                }
                guard.progress()
            };
            progress(snapshot);

            let truncated = response.finish == FinishReason::Length;
            if !still_missing.is_empty() {
                tracing::info!(translated = translated.len(), missing = still_missing.len(), truncated, "requesting the remaining lines");
            }
            if translated.is_empty() {
                fruitless_rounds += 1;
                if truncated && still_missing.len() > 1 {
                    return self.translate_halves(units, still_missing, state, progress, cancel).await;
                }
            } else {
                fruitless_rounds = 0;
            }
            pending = still_missing;
        }
        Ok(pending)
    }

    async fn translate_halves(
        &self,
        units: &[TranslationUnit],
        mut unit_ids: Vec<usize>,
        state: &Arc<Mutex<SharedState>>,
        progress: &ProgressCallback,
        cancel: &CancellationToken,
    ) -> Result<Vec<usize>> {
        let second_half = unit_ids.split_off(unit_ids.len() / 2);
        let mut failed = Box::pin(self.translate_batch(units, unit_ids, state, progress, cancel)).await?;
        failed.extend(Box::pin(self.translate_batch(units, second_half, state, progress, cancel)).await?);
        Ok(failed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::config::{ModelSelection, ProviderKind};
    use crate::llm::{ChatModel, ChatRequest, ChatResponse, LlmFuture};

    /// Answers each request with the next scripted reply, translating the first
    /// `lines` lines it was asked for.
    struct ScriptedModel {
        replies: Mutex<VecDeque<Result<(usize, FinishReason), LlmError>>>,
        requests: Mutex<Vec<String>>,
    }

    impl ChatModel for ScriptedModel {
        fn complete<'a>(&'a self, request: ChatRequest) -> LlmFuture<'a, ChatResponse> {
            Box::pin(async move {
                let message = request.messages[0].text.clone();
                self.requests.lock().push(message.clone());
                let (lines, finish) = self.replies.lock().pop_front().expect("unexpected request")?;
                let text: String = message
                    .lines()
                    .filter(|line| line.starts_with('#'))
                    .take(lines)
                    .map(|line| format!("{}\n", line.replace(": line", ": Zeile")))
                    .collect();
                request.stream_text(&text);
                Ok(ChatResponse { text, usage: TokenUsage { input_tokens: 10, output_tokens: 5, ..Default::default() }, finish })
            })
        }

        fn describe(&self) -> String {
            "scripted".into()
        }
    }

    fn translate(replies: Vec<Result<(usize, FinishReason), LlmError>>, unit_count: usize) -> (TranslationOutcome, Vec<String>, Vec<TranslationProgress>) {
        let model = Arc::new(ScriptedModel { replies: Mutex::new(replies.into()), requests: Mutex::new(Vec::new()) });
        let handle = ModelHandle { client: model.clone(), selection: ModelSelection::default(), provider_kind: ProviderKind::OpenAi, max_retries: 0 };
        let units: Vec<TranslationUnit> =
            (0..unit_count).map(|id| TranslationUnit { id, text: format!("line {id}"), occurrences: vec![id], parts: 1 }).collect();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let progress: ProgressCallback = Arc::new(move |p| sink.lock().push(p));
        let translator = Translator::new(handle, TranslationConfig::default(), "system".into());
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let outcome = runtime.block_on(translator.translate(&units, progress, &CancellationToken::new())).unwrap();
        let requests = model.requests.lock().clone();
        let reports = reports.lock().clone();
        (outcome, requests, reports)
    }

    #[test]
    fn whole_file_goes_out_in_one_request() {
        let (outcome, requests, progress) = translate(vec![Ok((50, FinishReason::Stop))], 50);
        assert_eq!(requests.len(), 1);
        assert_eq!(outcome.translations.len(), 50);
        assert_eq!(outcome.translations[&7], "Zeile 7");
        assert!(outcome.failed_unit_ids.is_empty());
        assert_eq!(progress.last().unwrap().units_done, 50);
    }

    #[test]
    fn long_file_goes_out_in_batches() {
        let (outcome, requests, _) = translate((0..3).map(|_| Ok((100, FinishReason::Stop))).collect(), 250);
        assert_eq!(requests.len(), 3);
        assert_eq!(outcome.translations.len(), 250);
        assert!(outcome.failed_unit_ids.is_empty());
    }

    #[test]
    fn truncated_answer_continues_where_it_stopped() {
        let (outcome, requests, _) = translate(vec![Ok((30, FinishReason::Length)), Ok((20, FinishReason::Stop))], 50);
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("Continuity") && requests[1].contains("line 29 => Zeile 29"));
        assert!(requests[1].contains("Translate these 20 lines") && !requests[1].contains("#29:"));
        assert_eq!(outcome.translations.len(), 50);
        assert_eq!(outcome.usage.input_tokens, 20);
    }

    #[test]
    fn oversized_batch_is_split_and_silent_lines_give_up() {
        let replies = vec![
            Err(LlmError::ContextTooLong("too long".into())),
            Ok((5, FinishReason::Stop)),
            Ok((0, FinishReason::Stop)),
            Ok((0, FinishReason::Stop)),
            Ok((0, FinishReason::Stop)),
        ];
        let (outcome, requests, _) = translate(replies, 10);
        assert_eq!(requests.len(), 5);
        assert_eq!(outcome.translations.len(), 5);
        assert_eq!(outcome.failed_unit_ids, vec![5, 6, 7, 8, 9]);
    }
}
