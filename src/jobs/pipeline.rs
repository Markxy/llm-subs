//! The stages one job goes through, in order. Each stage reports to the job handle so
//! the UI can show what is happening and why something was skipped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::config::{find_language, language_name, Config, Language, OutputMode};
use crate::ffmpeg::{self, FfmpegTools, MediaInfo};
use crate::llm::{build_model, LlmServices};
use crate::media::{embed_subtitle_track, find_sidecar_subtitles, select_source, sidecar_path_for_language, SubtitleSource};
use crate::subtitles::{parse_srt, serialize_srt, Cue};
use crate::translation::{assemble_translated_cues, build_units, system_prompt, PromptContext, Translator};

use super::job::FileFingerprint;
use super::{JobHandle, JobInput, JobStage};

pub struct PipelineContext {
    pub config: Arc<Config>,
    pub tools: Option<FfmpegTools>,
    pub llm: LlmServices,
}

#[derive(Debug)]
pub enum PipelineOutcome {
    Completed { output_path: PathBuf },
    /// `no_usable_source` means there was nothing to translate from yet, so a subtitle
    /// file that turns up later is worth another try.
    Skipped { reason: String, no_usable_source: bool },
}

impl PipelineOutcome {
    fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped { reason: reason.into(), no_usable_source: false }
    }
}

struct PreparedSource {
    media_info: Option<MediaInfo>,
    source: SubtitleSource,
    output_path: PathBuf,
}

pub async fn run_pipeline(job: &JobHandle, ctx: &PipelineContext) -> Result<PipelineOutcome> {
    let config = ctx.config.clone();
    let target = find_language(&config.general.target_language)
        .with_context(|| format!("unknown target language '{}'", config.general.target_language))?;
    let cancel = job.cancel_token();
    let input = job.input();
    let force = job.options().force;

    job.set_stage(JobStage::Probing, "Inspecting the file");
    let prepared = match prepare_source(job, ctx, &input, target, force).await? {
        Ok(prepared) => prepared,
        Err(skipped) => return Ok(skipped),
    };
    if config.output.mode == OutputMode::Sidecar && prepared.output_path.exists() && config.translation.skip_if_target_exists && !force {
        return Ok(PipelineOutcome::skipped(format!("{} already exists (use Translate again to replace it)", prepared.output_path.display())));
    }
    job.update(|record| record.source_description = Some(prepared.source.describe()));
    job.info(format!("Source: {}", prepared.source.describe()));

    job.set_stage(JobStage::Extracting, "Reading subtitles");
    let cues = load_cues(ctx, &input, &prepared.source, &cancel).await?;
    job.update(|record| record.cue_count = cues.len());
    job.info(format!("{} subtitle cues", cues.len()));

    job.set_stage(JobStage::Translating, "Translating");
    let units = build_units(&cues);
    if units.is_empty() {
        bail!("the subtitles contain no translatable text");
    }
    job.info(format!("{} distinct lines to translate", units.len()));
    let translation_model = build_model(&config, &config.models.translation, &ctx.llm).context("translation model")?;
    job.info(format!("Model: {}", translation_model.describe()));
    let source_language_name = prepared.source.language().map(language_name);
    let system = system_prompt(&PromptContext {
        target_language_name: target.english_name,
        source_language_name: source_language_name.as_deref(),
        extra_instructions: &config.translation.extra_instructions,
    });
    let translator = Translator::new(translation_model.clone(), config.translation.clone(), system);
    let progress_job = job.clone();
    let outcome = translator
        .translate(
            &units,
            Arc::new(move |progress| {
                let fraction = if progress.units_total == 0 { 1.0 } else { progress.units_done as f32 / progress.units_total as f32 };
                progress_job.set_stage_progress(fraction, format!("Translated {} of {} lines", progress.units_done, progress.units_total));
            }),
            &cancel,
        )
        .await?;
    let cost = translation_model.cost_of(&outcome.usage);
    job.info(format!(
        "{} request{}, {}{}",
        outcome.requests,
        if outcome.requests == 1 { "" } else { "s" },
        outcome.usage.describe(),
        cost.map(|cost| format!(", {}", cost.describe())).unwrap_or_default()
    ));
    if !outcome.failed_unit_ids.is_empty() {
        job.warn(format!("{} lines never came back from the model and keep their source text", outcome.failed_unit_ids.len()));
    }

    job.set_stage(JobStage::Writing, "Writing subtitles");
    let (translated, report) = assemble_translated_cues(&cues, &units, &outcome.translations, &config.translation.line_wrap);
    let srt_text = serialize_srt(&translated);
    write_text_file(&prepared.output_path, &srt_text)?;
    job.info(format!("Wrote {}", prepared.output_path.display()));

    let mut final_output = prepared.output_path.clone();
    if config.output.mode == OutputMode::Embed
        && let (Some(info), JobInput::Media(media_path)) = (&prepared.media_info, &input)
    {
        let tools = ctx.tools.as_ref().context("FFmpeg is required to embed subtitles")?;
        job.set_stage_progress(0.5, "Adding the subtitle track to the media file");
        let title = format!("{} (LLM Subs)", target.english_name);
        embed_subtitle_track(tools, media_path, info, &prepared.output_path, target, &title, &cancel).await?;
        job.info("Subtitle track embedded");
        // The rewritten file looks new to a folder watcher; this run already covers it.
        let fingerprint = FileFingerprint::of(media_path);
        job.update(|record| record.input_fingerprint = fingerprint);
        if !config.output.keep_sidecar_when_embedding {
            let _ = tokio::fs::remove_file(&prepared.output_path).await;
        }
        final_output = media_path.clone();
    }

    job.update(|record| {
        record.translated_cues = report.translated_cues;
        record.untranslated_cues = report.untranslated_cues;
        record.usage = outcome.usage;
        record.requests = outcome.requests;
        record.cost = cost;
        record.output_path = Some(final_output.clone());
    });
    Ok(PipelineOutcome::Completed { output_path: final_output })
}

/// Finds what to translate from and where the result goes. `Err(outcome)` means skip.
async fn prepare_source(
    job: &JobHandle,
    ctx: &PipelineContext,
    input: &JobInput,
    target: &Language,
    force: bool,
) -> Result<Result<PreparedSource, PipelineOutcome>> {
    let config = &ctx.config;
    match input {
        JobInput::Media(path) => {
            let tools = ctx.tools.as_ref().context("FFmpeg is not available; install it or download it in Setup")?;
            let info = ffmpeg::probe(tools, path, &job.cancel_token()).await.context("probing media")?;
            let sidecars = find_sidecar_subtitles(path, &config.watch);
            let report = select_source(Some(&info), &sidecars, target.code, &config.translation.preferred_source_languages);
            for note in &report.notes {
                job.info(note.clone());
            }
            if let Some(existing) = report.target_already_present
                && config.translation.skip_if_target_exists
                && !force
            {
                return Ok(Err(PipelineOutcome::skipped(format!("{} subtitles already present: {existing}", target.english_name))));
            }
            let Some(source) = report.chosen else {
                let reason = if info.subtitle_streams.is_empty() && sidecars.is_empty() {
                    "no subtitles found in or next to the media".to_string()
                } else {
                    "no usable text subtitles (bitmap tracks need OCR)".to_string()
                };
                return Ok(Err(PipelineOutcome::Skipped { reason, no_usable_source: true }));
            };
            let output_path = sidecar_path_for_language(path, target.code);
            Ok(Ok(PreparedSource { media_info: Some(info), source, output_path }))
        }
        JobInput::Subtitle(path) => {
            let (base_stem, language) = split_language_suffix(path);
            if language.as_deref().is_some_and(|l| crate::config::same_language(l, target.code)) && !force {
                return Ok(Err(PipelineOutcome::skipped(format!("file is already in {}", target.english_name))));
            }
            let output_path = path.with_file_name(format!("{base_stem}.{}.srt", target.code));
            Ok(Ok(PreparedSource {
                media_info: None,
                source: SubtitleSource::Sidecar { path: path.clone(), language },
                output_path,
            }))
        }
    }
}

/// `Movie.en` -> (`Movie`, Some("en")); `Movie` -> (`Movie`, None).
fn split_language_suffix(path: &Path) -> (String, Option<String>) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("subtitles").to_string();
    if let Some((base, suffix)) = stem.rsplit_once('.')
        && let Some(language) = find_language(suffix)
        && !base.is_empty()
    {
        return (base.to_string(), Some(language.code.to_string()));
    }
    (stem, None)
}

async fn load_cues(
    ctx: &PipelineContext,
    input: &JobInput,
    source: &SubtitleSource,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<Cue>> {
    let srt_text = match (source, input) {
        (SubtitleSource::Embedded(stream), JobInput::Media(media_path)) => {
            let tools = ctx.tools.as_ref().context("FFmpeg is required to extract subtitles")?;
            ffmpeg::extract_subtitle_stream(tools, media_path, stream, cancel).await.context("extracting subtitle track")?
        }
        (SubtitleSource::Sidecar { path, .. }, _) => ffmpeg::read_subtitle_file(ctx.tools.as_ref(), path, cancel).await?,
        (SubtitleSource::Embedded(_), JobInput::Subtitle(_)) => bail!("embedded stream selected for a subtitle file"),
    };
    parse_srt(&srt_text).context("parsing subtitles")
}

fn write_text_file(path: &Path, text: &str) -> Result<()> {
    crate::config::store::write_atomically(path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_language_suffix_from_subtitle_names() {
        assert_eq!(split_language_suffix(Path::new("/m/Movie.en.srt")), ("Movie".to_string(), Some("en".to_string())));
        assert_eq!(split_language_suffix(Path::new("/m/Movie.srt")), ("Movie".to_string(), None));
        assert_eq!(split_language_suffix(Path::new("/m/Show.S01E02.srt")), ("Show.S01E02".to_string(), None));
    }
}
