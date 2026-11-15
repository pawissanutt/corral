//! Typed snapshots remain typed through notification assembly.
use anyhow::{Result, bail};
use corral_ops::{Ctx, app_read};
use corral_protocol::{
    assistant::ThreadEntry,
    board::BoardCursor,
    rows::*,
    subscription::{LiveText, RowsDelta, SubDelta, Topic},
};

#[derive(PartialEq)]
pub(crate) enum Rows {
    Inbox(Vec<InboxRow>),
    Plans(Vec<PlanRow>),
    Agents(Vec<AgentRow>),
    Assistant(Vec<AssistantRow>),
    Usage(Vec<UsageRow>),
    Settings(Vec<SettingsSnapshot>),
    Thread(Vec<ThreadEntry>),
}

macro_rules! each {
    ($self:expr, $rows:ident, $body:expr) => {
        match $self {
            Rows::Inbox($rows) => $body,
            Rows::Plans($rows) => $body,
            Rows::Agents($rows) => $body,
            Rows::Assistant($rows) => $body,
            Rows::Usage($rows) => $body,
            Rows::Settings($rows) => $body,
            Rows::Thread($rows) => $body,
        }
    };
}

impl Rows {
    pub fn len(&self) -> usize {
        each!(self, rows, rows.len())
    }
    pub fn id(&self, index: usize) -> &str {
        each!(self, rows, rows[index].id.as_str())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn delta(
        &self,
        epoch: u64,
        cursor: BoardCursor,
        range: std::ops::Range<usize>,
        left: Vec<String>,
        more: bool,
        scope: Option<&str>,
        live: Option<LiveText>,
    ) -> SubDelta {
        macro_rules! page {
            ($rows:expr) => {
                RowsDelta {
                    epoch,
                    rows: $rows[range].to_vec(),
                    left,
                    cursor,
                    more,
                }
            };
        }
        match self {
            Self::Inbox(rows) => SubDelta::Inbox { delta: page!(rows) },
            Self::Plans(rows) => SubDelta::Plans { delta: page!(rows) },
            Self::Agents(rows) => SubDelta::Agents { delta: page!(rows) },
            Self::Assistant(rows) => SubDelta::Assistant { delta: page!(rows) },
            Self::Usage(rows) => SubDelta::Usage { delta: page!(rows) },
            Self::Settings(rows) => SubDelta::Settings { delta: page!(rows) },
            Self::Thread(rows) => SubDelta::Thread {
                scope: scope.unwrap_or_default().to_owned(),
                delta: page!(rows),
                live,
            },
        }
    }
}

pub(super) fn read(ctx: &Ctx, topic: Topic) -> Result<Rows> {
    Ok(match topic {
        Topic::Inbox => Rows::Inbox(app_read::inbox_rows(ctx)?),
        Topic::Plans => Rows::Plans(app_read::plans_rows(ctx)?),
        Topic::Agents => Rows::Agents(app_read::agents_rows(ctx)?),
        Topic::Assistant => Rows::Assistant(app_read::assistant_rows(ctx)?),
        Topic::Usage => Rows::Usage(app_read::usage_rows(ctx)?),
        Topic::Settings => Rows::Settings(app_read::settings_rows(ctx)?),
        // A watched thread publishes freshness only. Its history is fetched one bounded
        // canonical page at a time, never materialized as an all-history snapshot.
        Topic::Thread => Rows::Thread(Vec::new()),
        _ => bail!("unsupported app topic"),
    })
}
