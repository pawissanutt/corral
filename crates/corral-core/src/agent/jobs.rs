//! Background jobs of a Pi-native worker (T-565): the durable identity of each process the host
//! started and the monitors registered on it. Output files are runtime caches; these records
//! are what recovery, fencing and run end use to find and reap owned process groups.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Jobs a container keeps, live or ended; the oldest ended job is evicted first.
pub const JOBS_MAX: usize = 64;
pub const JOB_MONITORS_MAX: usize = 16;
pub const JOB_COMMAND_MAX_BYTES: usize = 4096;
pub const JOB_PATTERN_MAX_BYTES: usize = 512;
pub const JOB_LINE_MAX_BYTES: usize = 1024;
pub const JOB_DETAIL_MAX_BYTES: usize = 512;

/// Resolved `[agent.capabilities.worker.background_shell]` bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JobSettings {
    /// Jobs that may be prepared or running at once.
    pub max_jobs: u32,
    /// Output bytes one job retains; later output is counted and dropped.
    pub max_output_bytes: u64,
    /// Wall-clock limit after which the host kills the job's process group.
    pub max_runtime_secs: u64,
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            max_jobs: 4,
            max_output_bytes: 8 * 1024 * 1024,
            max_runtime_secs: 3600,
        }
    }
}

impl JobSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_jobs == 0 || self.max_jobs > 16 {
            return Err("background_shell.max_jobs must be between 1 and 16".into());
        }
        if self.max_output_bytes < 1024 || self.max_output_bytes > 1024 * 1024 * 1024 {
            return Err(
                "background_shell.max_output_bytes must be between 1024 and 1073741824".into(),
            );
        }
        if self.max_runtime_secs == 0 || self.max_runtime_secs > 7 * 24 * 3600 {
            return Err("background_shell.max_runtime_secs must be between 1 and 604800".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Recorded before spawn; no process identity is known yet.
    Prepared,
    Running,
    /// The host observed the exit.
    Exited,
    /// Corral killed the verified process group after a fence, host loss or run end.
    Reaped,
    /// Ended without a verifiable process: identity unknown or no longer matching.
    Lost,
}

impl JobState {
    pub fn live(self) -> bool {
        matches!(self, JobState::Prepared | JobState::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorOn {
    Exit,
    Line,
}

/// One `corral_job_monitor` registration; `wait` names its run wait and wake subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobMonitor {
    pub wait: String,
    pub on: MonitorOn,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    pub request_id: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub generation: u32,
    pub command: String,
    pub state: JobState,
    /// Process group leader; jobs start in their own group, so the group ID is this pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Kernel start time of `pid`; a pid whose start time differs is another process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    pub prepared_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The whole process group is verified gone, not only its leader: the host's exit report
    /// says so after its sweep, or the reaper sets it after killing what remained.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub group_clear: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<JobMonitor>,
}

impl JobRecord {
    /// Live, or exited with a known group that nobody has verified empty.
    pub fn needs_reap(&self) -> bool {
        self.state.live()
            || (self.state == JobState::Exited && self.pid.is_some() && !self.group_clear)
    }
}

/// Evicts ended jobs, oldest first, until the container is within `JOBS_MAX`.
pub fn evict_jobs(jobs: &mut Vec<JobRecord>) {
    while jobs.len() > JOBS_MAX {
        let Some(oldest) = jobs.iter().position(|job| !job.needs_reap()) else {
            return;
        };
        jobs.remove(oldest);
    }
}

pub fn bounded_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
