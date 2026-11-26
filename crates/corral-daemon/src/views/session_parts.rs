//! Session details are host projections, with the host's own continuation tokens.
use anyhow::{Context, Result, ensure};
use corral_engine::Engine;
use corral_protocol::detail::*;
use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::{details, fingerprint};
use crate::{
    sessions::{
        self,
        forward::{HostCall, PartTooLarge, host_call},
    },
    state::Shared,
};

const PAGE_TOKEN_BYTES: usize = 128;
const BRIEF_PAGE_BYTES: usize = 40 * 1024;

#[derive(Default, Serialize)]
struct Args<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison: Option<ChangesComparison>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
}

#[derive(Deserialize, Serialize)]
struct Page<T> {
    #[serde(flatten)]
    body: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct ChangeFile {
    path: String,
    patch: String,
    truncated: bool,
}

#[derive(Deserialize, Serialize)]
struct Events {
    #[serde(flatten)]
    body: SessionEventsPage,
    next: u64,
    more: bool,
}

pub(super) fn read(
    shared: &Shared,
    engine: &Engine,
    query: &DetailGet,
) -> Result<Option<DetailReply>> {
    let after = query.page.as_deref();
    let check_after = || {
        ensure!(
            after.is_none_or(|token| token.len() <= PAGE_TOKEN_BYTES),
            "invalid: session part page token exceeds its byte bound"
        );
        Ok::<(), anyhow::Error>(())
    };
    macro_rules! unpaged {
        ($id:expr, $method:literal, $args:expr, $body:path) => {{
            ensure!(
                after.is_none(),
                "invalid: this session part does not paginate"
            );
            (
                $body(host_call(shared, $id, $method, $args, HostCall::Read)?),
                None,
            )
        }};
    }
    macro_rules! paged {
        ($id:expr, $method:literal, $args:expr, $convert:expr) => {{
            check_after()?;
            let Page { body, next } = host_call(shared, $id, $method, $args, HostCall::Read)?;
            (($convert)(body), next)
        }};
    }
    let args = || Args {
        after,
        ..Args::default()
    };
    let (body, next) = match &query.target {
        DetailRef::SessionCards { id } => paged!(id, "cards", args(), DetailBody::SessionCards),
        DetailRef::SessionModels { id } => paged!(id, "models", args(), DetailBody::SessionModels),
        DetailRef::SessionChild { id, child } => {
            ensure!(
                after.is_none(),
                "invalid: this session part does not paginate"
            );
            let parent = sessions::resolve(shared, id)?;
            (
                DetailBody::SessionChild(sessions::children::detail(&parent, child)?),
                None,
            )
        }
        DetailRef::SessionJobs { id } => {
            unpaged!(id, "jobs", Map::new(), DetailBody::SessionJobs)
        }
        DetailRef::SessionJobOutput { id, job } => paged!(
            id,
            "job_output",
            Args {
                job: Some(job),
                ..args()
            },
            DetailBody::SessionJobOutput
        ),
        DetailRef::SessionChanges {
            id,
            comparison,
            scope,
        } => paged!(
            id,
            "changes",
            Args {
                comparison: Some(*comparison),
                scope: scope.as_deref(),
                ..args()
            },
            DetailBody::SessionChanges
        ),
        DetailRef::SessionChangeFile {
            id,
            comparison,
            scope,
            path,
        } => paged!(
            id,
            "change_file",
            Args {
                comparison: Some(*comparison),
                scope: scope.as_deref(),
                path: Some(path),
                ..args()
            },
            |ChangeFile {
                 path,
                 patch,
                 truncated,
             }| {
                DetailBody::SessionChangeFile(PatchPage {
                    path,
                    patch,
                    truncated,
                    candidate: None,
                    head: None,
                })
            }
        ),
        DetailRef::SessionUsage { id } => {
            unpaged!(id, "usage", Map::new(), DetailBody::SessionUsage)
        }
        DetailRef::SessionContext { id } => {
            unpaged!(id, "context", Map::new(), DetailBody::SessionContext)
        }
        DetailRef::SessionPlan { id } => paged!(id, "plan", args(), DetailBody::SessionPlan),
        DetailRef::SessionEvents { id } => {
            check_after()?;
            events(shared, id, after)?
        }
        DetailRef::SessionCommands { id } => {
            unpaged!(id, "commands", Map::new(), DetailBody::SessionCommands)
        }
        DetailRef::BuildBoard { id } => unpaged!(id, "board", Map::new(), DetailBody::BuildBoard),
        DetailRef::BuildPlanTask { id, key } => {
            check_after()?;
            let (page, next, etag) = board_plan_task(shared, &query.target, id, key, after)?;
            return details::reply(
                query,
                engine.store.latest_seq()?,
                etag,
                DetailBody::BuildPlanTask(page),
                next,
            )
            .map(Some);
        }
        DetailRef::SessionPlanModeDoc { id, card } => {
            check_after()?;
            let (page, next, etag) = plan_mode_doc(shared, &query.target, id, card, after)?;
            return details::reply(
                query,
                engine.store.latest_seq()?,
                etag,
                DetailBody::SessionPlanModeDoc(page),
                next,
            )
            .map(Some);
        }
        DetailRef::BuildRecord { id } => {
            unpaged!(id, "record", Map::new(), DetailBody::BuildRecord)
        }
        _ => return Ok(None),
    };
    ensure!(
        next.as_ref()
            .is_none_or(|token| token.len() <= PAGE_TOKEN_BYTES),
        "Pi session part continuation exceeds its byte bound"
    );
    let etag = fingerprint(&(&query.target, after, &body, &next))?;
    details::reply(query, engine.store.latest_seq()?, etag, body, next).map(Some)
}

fn board_plan_task(
    shared: &Shared,
    target: &DetailRef,
    id: &str,
    key: &str,
    after: Option<&str>,
) -> Result<(BuildPlanTaskPage, Option<String>, String)> {
    #[derive(Serialize)]
    struct Task<'a> {
        key: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        after: Option<&'a str>,
    }
    let previous = after
        .map(|token| brief_cursor(token).context("invalid: malformed board task brief cursor"))
        .transpose()?;
    let Page { body, next }: Page<BuildPlanTaskPage> = host_call(
        shared,
        id,
        "board_plan_task",
        Task { key, after },
        HostCall::Read,
    )?;
    ensure!(body.key == key, "invalid Pi board task brief range");
    let range = (body.body.len(), body.bytes, body.offset);
    // A cursor binds to the host's fingerprint of the whole brief. Only a complete
    // one-page brief has no cursor on either side, and can be fingerprinted directly.
    let etag = match brief_range("board task brief", range, previous, next.as_deref())? {
        Some(stamp) => fingerprint(&(target, body.bytes, stamp))?,
        None => fingerprint(&(target, &body.body))?,
    };
    Ok((body, next, etag))
}

fn plan_mode_doc(
    shared: &Shared,
    target: &DetailRef,
    id: &str,
    card: &str,
    after: Option<&str>,
) -> Result<(PlanModeDocPage, Option<String>, String)> {
    #[derive(Serialize)]
    struct Doc<'a> {
        card: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        after: Option<&'a str>,
    }
    let previous = after
        .map(|token| brief_cursor(token).context("invalid: malformed plan document cursor"))
        .transpose()?;
    let Page { body, next }: Page<PlanModeDocPage> = host_call(
        shared,
        id,
        "plan_mode_doc",
        Doc { card, after },
        HostCall::Read,
    )?;
    let range = (body.body.len(), body.bytes, body.offset);
    let head = (target, &body.title, body.revision);
    let etag = match brief_range("plan document", range, previous, next.as_deref())? {
        Some(stamp) => fingerprint(&(head, body.bytes, stamp))?,
        None => fingerprint(&(head, &body.body))?,
    };
    Ok((body, next, etag))
}

/// Checks one `(len, bytes, offset)` page against its cursors, returning their shared stamp.
fn brief_range<'a>(
    what: &str,
    (len, bytes, offset): (usize, u64, u64),
    previous: Option<(&'a str, u64)>,
    next: Option<&'a str>,
) -> Result<Option<&'a str>> {
    ensure!(len <= BRIEF_PAGE_BYTES, PartTooLarge);
    let end = offset.checked_add(len as u64);
    ensure!(
        offset == previous.map_or(0, |(_, offset)| offset)
            && end.is_some_and(|end| end <= bytes)
            && (offset < bytes || bytes == 0),
        "invalid Pi {what} range"
    );
    let following = next
        .map(|token| brief_cursor(token).with_context(|| format!("invalid Pi {what} continuation")))
        .transpose()?;
    if let Some((stamp, start)) = following {
        ensure!(
            Some(start) == end
                && start > offset
                && start < bytes
                && previous.is_none_or(|(prior, _)| prior == stamp),
            "invalid Pi {what} continuation"
        );
    } else {
        ensure!(end == Some(bytes), "incomplete Pi {what}");
    }
    Ok(previous.or(following).map(|(stamp, _)| stamp))
}

fn brief_cursor(token: &str) -> Option<(&str, u64)> {
    let (stamp, offset) = token.split_once('.')?;
    if stamp.len() != 22
        || !stamp
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        || offset.starts_with('0')
        || !offset.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((stamp, offset.parse().ok()?))
}

fn events(shared: &Shared, id: &str, after: Option<&str>) -> Result<(DetailBody, Option<String>)> {
    #[derive(Serialize)]
    struct Since {
        seq: u64,
        detail: bool,
    }
    let seq = match after {
        Some(after) => {
            ensure!(
                after.bytes().all(|byte| byte.is_ascii_digit()),
                "invalid: session event cursor"
            );
            after.parse().context("invalid: session event cursor")?
        }
        None => 0,
    };
    let Events { body, next, more } = host_call(
        shared,
        id,
        "events_since",
        Since { seq, detail: true },
        HostCall::Read,
    )?;
    Ok((
        DetailBody::SessionEvents(body),
        more.then(|| next.to_string()),
    ))
}
