//! Typed capture schedules and frame-exact recording job requests.
use crate::tools::{DetailLevel, InspectInclude};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::{OpPath, Pid};

/// Operations on a bridge-owned asynchronous job.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum JobAction {
    /// Start a new job (default).
    #[default]
    Start,
    /// Observe progress/results; no advancement caused by polling.
    Status,
    /// Stop, finalize partial results, and release transport ownership.
    Cancel,
    /// Read a bounded chunk of a finalized record artifact.
    Read,
    /// Delete retained results and the private record artifact.
    Release,
}

/// Public reset signal owned by the network, not an arbitrary script.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetSignal {
    /// Stateful COMP exposing the reset Pulse.
    pub path: OpPath,
    /// Existing Pulse parameter (usually Reset).
    pub parameter: String,
}

/// Regular sampling schedule, compiled into explicit offsets.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepeatSchedule {
    /// Number of samples, 1..16.
    pub count: u32,
    /// Positive interval between samples, in frames.
    pub interval: u32,
}

/// Transport state after successful finalization. Failure always pauses.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AfterTiming {
    /// Remain paused (default).
    #[default]
    Pause,
    /// Resume playback from the resulting state.
    Play,
    /// Restore original play flags, never timeline/history.
    Restore,
}

/// Sequential forward schedule; never seeks backwards or skips intermediate cooks.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimingOptions {
    /// Effective output time source; omit to infer from source.time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_path: Option<OpPath>,
    /// Public component reset pulse, optional when continuing prepared state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<ResetSignal>,
    /// Initialization frames after reset (default 1 with reset, otherwise 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialize_frames: Option<u32>,
    /// Additional sequential warmup frames before offset zero; default 0.
    #[serde(default)]
    pub warmup_frames: u32,
    /// Strictly increasing offsets (1..16 samples, max offset 3600).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_frames: Option<Vec<u32>>,
    /// Regular schedule; mutually exclusive with sampleFrames/stepFrames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<RepeatSchedule>,
    /// Advance this many frames and capture once; 1..3600.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_frames: Option<u32>,
    /// Successful completion policy; default pause.
    #[serde(default)]
    pub after: AfterTiming,
    /// Local job deadline (1..600 seconds), default 120; not an RPC timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

/// Structural reads at the same controlled capture sample.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleInspect {
    /// 1..16 existing paths sharing the sampled clock.
    pub paths: Vec<OpPath>,
    /// Empty means nodes/errors/warnings. Content excluded for timed samples.
    #[serde(default)]
    pub include: Vec<InspectInclude>,
    /// Child roster detail.
    #[serde(default)]
    pub detail_level: DetailLevel,
}

/// Start/status/cancel/read/release for an N-frame TOP recording.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordParams {
    /// Target TD pid, including for status and retrieval.
    pub pid: Pid,
    /// Optional federation target.
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Job operation; default start.
    #[serde(default)]
    pub action: JobAction,
    /// Prior job id; omitted status/cancel discovers the active record job.
    #[serde(default)]
    pub job_id: Option<String>,
    /// Explicit TOP output, required on start (not a COMP viewer).
    #[serde(default)]
    pub path: Option<OpPath>,
    /// Base for relative operator paths.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Exact consecutive sample count, 1..3600, required on start.
    #[serde(default)]
    pub frames: Option<u32>,
    /// Reset, initialization, warmup and completion policy. No capture schedule.
    #[serde(default)]
    pub timing: Option<TimingOptions>,
    /// Artifact byte offset for read (default 0).
    #[serde(default)]
    pub offset: Option<u64>,
    /// Artifact read length (default/max 262144 bytes).
    #[serde(default)]
    pub length: Option<u32>,
}
