//! Parent-local readback index written by Pi; no path comes from a client request.
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Result, ensure};
use corral_protocol::{
    app::SESSION_PART_MAX,
    detail::SessionChildDetail,
    subscription::{LIVE_FLEET_BYTES, SessionFleet},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;

use super::{
    Session,
    forward::{HostCall, host_call_on},
    open_session_file, valid_child_id,
};
use crate::views::json_len;

const DIRECTORY: &str = "child-records";
// Individual records use the normal clipping of a host child part. Allow for JSON escaping on
// disk; the ordinary wire bound still applies to the projected detail below.
const RECORD_BYTES: usize = 256 * 1024;
const INTERRUPTED: &str = "Parent host stopped before the child recorded an outcome.";

#[derive(Deserialize)]
struct Record<T> {
    session_file: PathBuf,
    #[serde(rename = "detail")]
    data: T,
}

fn read<T: DeserializeOwned>(dir: &Path, path: &Path, limit: usize) -> Option<T> {
    let file = open_session_file(dir, path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}

fn record<T: DeserializeOwned>(dir: &Path, child: &str) -> Option<Record<T>> {
    if !valid_child_id(child) {
        return None;
    }
    let record: Record<T> = read(
        dir,
        &dir.join(DIRECTORY).join(format!("{child}.json")),
        RECORD_BYTES,
    )?;
    let path = &record.session_file;
    let name = path.file_name()?;
    let layout = path.parent()?.to_str()?;
    // Subscription web search uses the auxiliary directory too; only a host-written record
    // grants child readback. Never guess an auxiliary path from a caller-supplied UUID.
    let allowed = matches!(layout, "runtime/auxiliary" | "auxiliary")
        || (layout == "children" && name == format!("{child}.jsonl").as_str());
    (allowed
        && path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_))))
    .then_some(record)
}

fn readable(dir: &Path, path: &Path) -> Option<PathBuf> {
    let path = dir.join(path);
    open_session_file(dir, &path).ok()?;
    Some(path)
}

pub(super) fn transcript(dir: &Path, child: &str) -> Option<PathBuf> {
    let record = record::<serde::de::IgnoredAny>(dir, child)?;
    readable(dir, &record.session_file)
}

fn settle(state: &mut String, error: &mut Option<String>) {
    if matches!(state.as_str(), "queued" | "running" | "needs") {
        *state = "failed".into();
        *error = Some(INTERRUPTED.into());
    }
}

pub(crate) fn detail(parent: &Session, child: &str) -> Result<SessionChildDetail> {
    ensure!(valid_child_id(child), "invalid: expected a child id");
    let error = match host_call_on(parent, "child", json!({"child": child}), HostCall::Read) {
        Ok(detail) => return Ok(detail),
        Err(error) => error,
    };
    let retained = parent.dir.as_ref().and_then(|dir| {
        let dir = dir.canonicalize().ok()?;
        let record = record::<SessionChildDetail>(&dir, child)?;
        let mut detail = record.data;
        if detail.id != child {
            return None;
        }
        settle(&mut detail.state, &mut detail.error);
        detail.transcript = readable(&dir, &record.session_file).is_some();
        detail.activity = None;
        detail.pending_model = None;
        detail.question = None;
        detail.models.clear();
        detail.profiles.clear();
        Some(detail)
    });
    let detail = retained.ok_or(error)?;
    ensure!(
        json_len(&detail)? <= SESSION_PART_MAX,
        super::forward::PartTooLarge
    );
    Ok(detail)
}

pub(crate) fn fleet(parent: &Session) -> Option<SessionFleet> {
    let dir = parent.dir.as_ref()?.canonicalize().ok()?;
    let mut fleet: SessionFleet = read(&dir, &dir.join(DIRECTORY).join("fleet"), LIVE_FLEET_BYTES)?;
    fleet.children.retain(|child| valid_child_id(&child.id));
    for child in &mut fleet.children {
        settle(&mut child.state, &mut child.error);
        child.steerable = false;
        child.activity = None;
        child.pending_model = None;
        child.question = None;
    }
    (!fleet.children.is_empty()).then_some(fleet)
}
