//! Jobs: one translation of one file, from queue to finished record.

mod history;
mod job;
mod pipeline;
mod queue;

pub use history::JobHistory;
pub use job::{JobHandle, JobId, JobInput, JobLogLine, JobOptions, JobOrigin, JobRecord, JobStage, JobStatus, LogLevel};
pub use pipeline::{run_pipeline, PipelineContext, PipelineOutcome};
pub use queue::{JobQueue, QueueDependencies};
