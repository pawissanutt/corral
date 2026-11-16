//! Version-one in-session build records. These documents are not tasks and have no
//! scheduling, capacity or task-statistics projection. Wire timestamps stay verbatim.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// Every string union uses its wire spelling both in JSON and terminal reports.
macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }
        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
    };
}

wire_enum!(BuildKind { Build => "build", Assistant => "assistant" });
wire_enum!(OutcomeState { Open => "open", Landed => "landed", Kept => "kept", Abandoned => "abandoned" });
wire_enum!(SliceKind { Slice => "slice", Explore => "explore", Review => "review" });
wire_enum!(SliceState {
    Queued => "queued", Running => "running", Needs => "needs", Done => "done",
    Failed => "failed", Cancelled => "cancelled",
});
wire_enum!(GateResult { Pass => "pass", Fail => "fail", Skipped => "skipped" });
wire_enum!(FindingSeverity { Blocker => "blocker", Major => "major", Minor => "minor" });
wire_enum!(ResolutionState { Fixed => "fixed", Declined => "declined", Open => "open" });
wire_enum!(ReviewVerdict { Approve => "approve", Changes => "changes" });
wire_enum!(MessageDelivery {
    Delivered => "delivered", Queued => "queued", Undeliverable => "undeliverable",
    AnsweredFromReport => "answered_from_report",
});
wire_enum!(DecisionKind { Question => "question", Exit => "exit", Override => "override" });
wire_enum!(TimelinePhase {
    Started => "started", Children => "children", Verify => "verify", Review => "review",
    Land => "land", Exit => "exit",
});

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordUsage {
    pub input: u64,
    pub output: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordSliceUsage {
    #[serde(flatten)]
    pub usage: RecordUsage,
    pub turns: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordSlice {
    pub id: String,
    pub title: String,
    pub kind: SliceKind,
    pub owns: Vec<String>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub state: SliceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
    pub usage: RecordSliceUsage,
    pub outside: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordResolution {
    pub state: ResolutionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordFinding {
    pub id: String,
    pub severity: FindingSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<RecordResolution>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordReview {
    pub round: u64,
    pub reviewer: String,
    pub model: String,
    pub head: String,
    pub verdict: ReviewVerdict,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub findings: Vec<RecordFinding>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordMessage {
    pub at: String,
    pub from: String,
    pub to: String,
    pub text: String,
    pub delivery: MessageDelivery,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordDecision {
    pub at: String,
    pub kind: DecisionKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordIntent {
    pub at: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordOutcome {
    pub state: OutcomeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordDiff {
    pub commits: u64,
    pub files: u64,
    pub added: u64,
    pub removed: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordCommit {
    pub sha: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
}

/// The receipt fields retained by a record (without command lists or receipt paths).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordVerification {
    pub attempt: String,
    pub tier: String,
    pub head: String,
    pub result: GateResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command: Option<String>,
    pub started: String,
    pub ended: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordUsageTotals {
    pub parent: RecordUsage,
    pub children: RecordUsage,
    pub reviewers: RecordUsage,
    pub total: RecordUsage,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordTimeline {
    pub at: String,
    pub phase: TimelinePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordTranscripts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub children: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BuildRecord {
    pub version: u32,
    pub id: String,
    pub kind: BuildKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    pub intent: Vec<RecordIntent>,
    pub created: String,
    pub updated: String,
    pub repo_root: String,
    pub worktree: String,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub outcome: RecordOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<RecordDiff>,
    pub commits: Vec<RecordCommit>,
    pub slices: Vec<RecordSlice>,
    pub verification: Vec<RecordVerification>,
    pub reviews: Vec<RecordReview>,
    pub messages: Vec<RecordMessage>,
    pub decisions: Vec<RecordDecision>,
    pub usage: RecordUsageTotals,
    pub timeline: Vec<RecordTimeline>,
    pub transcripts: RecordTranscripts,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BuildRecordSummary {
    pub id: String,
    pub kind: BuildKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub title: String,
    pub created: String,
    pub updated: String,
    pub state: OutcomeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
    pub slices: usize,
    pub review_rounds: usize,
    pub gate_attempts: usize,
    pub gate_failures: usize,
    pub messages: usize,
    pub usage_total: RecordUsage,
    pub revision: u64,
}

impl BuildRecord {
    /// Checks the wire caps and every RFC 3339 timestamp without rewriting owner data.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("build record version must be 1".into());
        }
        validate_id(&self.id)?;
        if self.title.trim().is_empty() {
            return Err("title must not be blank".into());
        }
        capped_text("title", &self.title, 120)?;
        capped_count("intent", self.intent.len(), 32)?;
        capped_count("commits", self.commits.len(), 200)?;
        capped_count("messages", self.messages.len(), 500)?;
        timestamp("created", &self.created)?;
        timestamp("updated", &self.updated)?;
        if let Some(at) = &self.outcome.at {
            timestamp("outcome.at", at)?;
        }
        if self.outcome.state == OutcomeState::Landed
            && self
                .outcome
                .landed
                .as_ref()
                .is_none_or(|sha| sha.trim().is_empty())
        {
            return Err("landed outcome requires outcome.landed".into());
        }
        for intent in &self.intent {
            timestamp("intent.at", &intent.at)?;
            capped_text("intent.text", &intent.text, 4096)?;
        }
        for slice in &self.slices {
            if let Some(started) = &slice.started {
                timestamp("slice.started", started)?;
            }
            if let Some(ended) = &slice.ended {
                timestamp("slice.ended", ended)?;
            }
            if let Some(report) = &slice.report {
                capped_text("slice.report", report, 8192)?;
            }
        }
        for receipt in &self.verification {
            timestamp("verification.started", &receipt.started)?;
            timestamp("verification.ended", &receipt.ended)?;
        }
        for review in &self.reviews {
            timestamp("review.at", &review.at)?;
        }
        for message in &self.messages {
            timestamp("message.at", &message.at)?;
            capped_text("message.text", &message.text, 2048)?;
        }
        for decision in &self.decisions {
            timestamp("decision.at", &decision.at)?;
        }
        for entry in &self.timeline {
            timestamp("timeline.at", &entry.at)?;
        }
        Ok(())
    }

    pub fn summary(&self, revision: u64) -> BuildRecordSummary {
        BuildRecordSummary {
            id: self.id.clone(),
            kind: self.kind,
            project: self.project.clone(),
            title: self.title.clone(),
            created: self.created.clone(),
            updated: self.updated.clone(),
            state: self.outcome.state,
            landed: self.outcome.landed.clone(),
            override_reason: self.outcome.override_reason.clone(),
            slices: self.slices.len(),
            review_rounds: self.reviews.len(),
            gate_attempts: self.verification.len(),
            gate_failures: self
                .verification
                .iter()
                .filter(|r| r.result == GateResult::Fail)
                .count(),
            messages: self.messages.len(),
            usage_total: self.usage.total.clone(),
            revision,
        }
    }
}

pub fn validate_id(id: &str) -> Result<(), String> {
    let valid = id
        .as_bytes()
        .get(..2)
        .is_some_and(|prefix| prefix == b"A-" || prefix == b"B-")
        && (3..=83).contains(&id.len())
        && id.as_bytes()[2].is_ascii_alphanumeric()
        && id.as_bytes()[2..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid build record id {id:?}: expected ^[AB]-[a-z0-9][a-z0-9-]{{0,80}}$"
        ))
    }
}

fn capped_count(field: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        Err(format!("{field} exceeds {max} entries"))
    } else {
        Ok(())
    }
}

fn capped_text(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.chars().take(max + 1).count() > max {
        Err(format!("{field} exceeds {max} characters"))
    } else {
        Ok(())
    }
}

fn timestamp(field: &str, value: &str) -> Result<(), String> {
    // Timestamp's Temporal parser accepts extensions that RFC 3339 does not. Require
    // the RFC shape first, then let jiff check calendar dates, clock fields and offsets.
    let bytes = value.as_bytes();
    let shape = bytes.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b't')
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[..19]
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10 | 13 | 16) || b.is_ascii_digit());
    let suffix = if shape { &value[19..] } else { "" };
    let zone = if let Some(fraction) = suffix.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 { "" } else { &fraction[digits..] }
    } else {
        suffix
    };
    let zone_bytes = zone.as_bytes();
    let offset = matches!(zone, "Z" | "z")
        || (zone_bytes.len() == 6
            && matches!(zone_bytes[0], b'+' | b'-')
            && zone_bytes[3] == b':'
            && zone_bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 0 || i == 3 || b.is_ascii_digit()));
    if !shape || !offset || value.parse::<jiff::Timestamp>().is_err() {
        return Err(format!("{field} must be an RFC 3339 timestamp: {value:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> BuildRecord {
        serde_json::from_value(serde_json::json!({
            "version": 1, "id": "B-a", "kind": "build", "title": "Fix record storage",
            "intent": [], "created": "2026-11-15T10:00:00Z", "updated": "2026-11-15T10:00:00Z",
            "repo_root": "/repo", "worktree": "/repo/worktree", "branch": null, "base": null,
            "outcome": {"state": "open"}, "commits": [], "slices": [], "verification": [],
            "reviews": [], "messages": [], "decisions": [], "timeline": [],
            "usage": {
                "parent": {"input": 0, "output": 0}, "children": {"input": 0, "output": 0},
                "reviewers": {"input": 0, "output": 0}, "total": {"input": 0, "output": 0}
            },
            "transcripts": {"children": {}}
        }))
        .unwrap()
    }

    #[test]
    fn validation_refuses_invalid_identity_caps_timestamps_and_landing() {
        let valid = record();
        assert!(valid.validate().is_ok());
        for id in ["", "B-", "T-a", "B--a", "B-A", "B-a/../b", "B-é"] {
            let mut invalid = valid.clone();
            invalid.id = id.into();
            assert!(invalid.validate().unwrap_err().contains("id"), "{id}");
        }
        let mut invalid = valid.clone();
        invalid.id = format!("A-{}", "a".repeat(82));
        assert!(invalid.validate().unwrap_err().contains("id"));
        invalid.id.pop();
        assert!(invalid.validate().is_ok());
        invalid.version = 2;
        assert!(invalid.validate().unwrap_err().contains("version"));

        let mut invalid = valid.clone();
        invalid.title = " ".into();
        assert!(invalid.validate().unwrap_err().contains("title"));
        invalid.title = "界".repeat(120);
        assert!(invalid.validate().is_ok());
        invalid.title.push('界');
        assert!(invalid.validate().unwrap_err().contains("title"));

        let mut invalid = valid.clone();
        invalid.outcome.state = OutcomeState::Landed;
        assert!(invalid.validate().unwrap_err().contains("outcome.landed"));
        invalid.outcome.landed = Some(" ".into());
        assert!(invalid.validate().is_err());
        invalid.outcome.landed = Some("abc123".into());
        assert!(invalid.validate().is_ok());

        for at in [
            "not-a-date",
            "2026-02-30T00:00:00Z",
            "2026-11-15 00:00:00Z",
            "2026-11-15T00:00:00Z[UTC]",
            "2026-11-15T00:00:00",
        ] {
            let mut invalid = valid.clone();
            invalid.updated = at.into();
            assert!(invalid.validate().unwrap_err().contains("updated"), "{at}");
        }
        let mut invalid = valid.clone();
        invalid.intent = vec![RecordIntent {
            at: valid.created.clone(),
            text: "x".repeat(4097),
        }];
        assert!(invalid.validate().unwrap_err().contains("intent.text"));
        invalid.intent = vec![
            RecordIntent {
                at: valid.created.clone(),
                text: "x".into()
            };
            33
        ];
        assert!(invalid.validate().unwrap_err().contains("intent"));
        invalid = valid.clone();
        invalid.commits = vec![
            RecordCommit {
                sha: "abc".into(),
                subject: "fix".into(),
                slice: None
            };
            201
        ];
        assert!(invalid.validate().unwrap_err().contains("commits"));
        invalid = valid.clone();
        invalid.messages = vec![RecordMessage {
            at: valid.created.clone(),
            from: "parent".into(),
            to: "S1".into(),
            text: "x".repeat(2049),
            delivery: MessageDelivery::Delivered,
        }];
        assert!(invalid.validate().unwrap_err().contains("message.text"));
        invalid.messages[0].text = "x".into();
        invalid.messages = vec![invalid.messages[0].clone(); 501];
        assert!(invalid.validate().unwrap_err().contains("messages"));
        invalid = valid.clone();
        invalid.slices = vec![RecordSlice {
            id: "S1".into(),
            title: "Fix".into(),
            kind: SliceKind::Slice,
            owns: vec![],
            model: "provider/model".into(),
            effort: None,
            state: SliceState::Done,
            started: None,
            ended: None,
            usage: RecordSliceUsage {
                usage: RecordUsage {
                    input: 0,
                    output: 0,
                    cost: None,
                },
                turns: 0,
            },
            outside: vec![],
            report: Some("x".repeat(8193)),
        }];
        assert!(invalid.validate().unwrap_err().contains("slice.report"));
    }
}
