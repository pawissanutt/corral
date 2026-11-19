//! Session details are host projections, with the host's own continuation tokens.
use anyhow::{Context, Result, ensure};
use corral_engine::Engine;
use corral_protocol::detail::*;
use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::{details, fingerprint};
use crate::{
    sessions::forward::{HostCall, host_call},
    state::Shared,
};

const PAGE_TOKEN_BYTES: usize = 128;

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
            #[derive(Serialize)]
            struct Child<'a> {
                child: &'a str,
            }
            unpaged!(id, "child", Child { child }, DetailBody::SessionChild)
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
