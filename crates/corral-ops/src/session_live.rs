//! Read-only access to a session host's ephemeral live ring. No input or store writes.
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use corral_core::{
    TaskRef,
    agent::{self, ContainerRef, HostHealth, pm_state},
};
use corral_protocol::subscription::{
    SessionFleet, SessionLiveKind, SessionLiveToolState, SessionLiveTurnState,
};

use crate::{
    Ctx,
    agent::{
        bridge::{BridgeClient, BridgeError, LivePage},
        launch,
    },
};

pub const LIVE_IO_TIMEOUT: Duration = Duration::from_millis(250);
pub const LIVE_HOST_EVENTS: usize = 512;
pub const LIVE_EVENT_BYTES: usize = 8 * 1024;
pub const LIVE_PAYLOAD_BYTES: usize = 1024;
pub const LIVE_ERROR_BYTES: usize = 512;
pub const LIVE_FLEET_BYTES: usize = 44 * 1024;

#[derive(PartialEq, Eq)]
struct Host {
    generation: u32,
    dir: PathBuf,
}

/// One shared reader per subscribed session. Keeping the connection and hello result
/// makes an idle tick exactly one `live_since` call, not a state/hello round trip.
#[derive(Default)]
pub struct LiveReader {
    scope: Option<String>,
    identity: Option<ContainerRef>,
    host: Option<Host>,
    bridge: Option<BridgeClient>,
    seq: u64,
}

impl LiveReader {
    pub fn read(&mut self, ctx: &Ctx, id: &str) -> Result<LivePage> {
        if self.scope.as_deref() != Some(id) {
            *self = Self {
                scope: Some(id.to_owned()),
                ..Self::default()
            };
        }
        let result = self.read_inner(ctx, id).map_err(live_failure);
        if result.is_err() {
            self.host = None;
            self.bridge = None;
            self.seq = 0;
        }
        result
    }

    fn read_inner(&mut self, ctx: &Ctx, id: &str) -> Result<LivePage> {
        if self.identity.is_none() {
            let (_, container, _) = crate::app_read::agents::control_identity(ctx, id)
                .context("resolving live session identity")?;
            self.identity = Some(container.ok_or_else(no_bridge)?);
        }
        let container = self
            .identity
            .as_ref()
            .context("live identity unavailable")?;
        let host = resolve(ctx, container)?;
        let rebound = self.host.as_ref().is_some_and(|previous| previous != &host);
        let previous_seq = self.seq;
        if rebound {
            self.seq = 0;
        }
        if self.host.as_ref() != Some(&host) || self.bridge.is_none() {
            let bridge = BridgeClient::connect_dir(&host.dir, host.generation)
                .context("connecting live session bridge")?;
            bridge.set_io_timeout(LIVE_IO_TIMEOUT)?;
            self.bridge = Some(bridge);
            self.host = Some(host);
        }
        let mut page = self
            .bridge
            .as_mut()
            .context("live bridge unavailable")?
            .live_since(self.seq)
            .context("reading live session page")?;
        let host = self.host.as_ref().context("live host unavailable")?;
        ensure!(
            page.generation == host.generation,
            "live page names another generation"
        );
        bound_page_fleet(&mut page)?;
        if rebound {
            reconcile_rebind(&mut page, previous_seq);
        }
        validate(&page, self.seq)?;
        self.seq = page.next;
        Ok(page)
    }
}

/// Preserve field presence: Some(empty) clears the daemon cache; None carries no update.
fn bound_page_fleet(page: &mut LivePage) -> Result<()> {
    if let Some(fleet) = &mut page.fleet {
        bound_fleet(fleet, LIVE_FLEET_BYTES)?;
    }
    Ok(())
}

/// A new bridge starts a fresh ring even when the watched session keeps its generation.
fn reconcile_rebind(page: &mut LivePage, after: u64) {
    page.gap = Some(corral_protocol::subscription::SessionLiveGap {
        after,
        resumed: page
            .events
            .first()
            .map_or(page.next.saturating_add(1), |event| event.seq),
        missing: page.gap.as_ref().map_or(0, |gap| gap.missing),
        reconcile: true,
    });
}

/// Resolve a session id and read one bounded page without retaining a connection.
/// Producers should retain `LiveReader` instead so hello is negotiated only once.
pub fn read_page(ctx: &Ctx, id: &str, seq: u64) -> Result<LivePage> {
    let mut reader = LiveReader {
        scope: Some(id.to_owned()),
        seq,
        ..LiveReader::default()
    };
    reader.read(ctx, id)
}

fn no_bridge() -> anyhow::Error {
    BridgeError {
        code: "no_bridge".into(),
        reason: "session has no healthy bridged host".into(),
    }
    .into()
}

fn live_failure(error: anyhow::Error) -> anyhow::Error {
    if error
        .downcast_ref::<BridgeError>()
        .is_some_and(|error| matches!(error.code.as_str(), "no_bridge" | "live_unsupported"))
    {
        error
    } else {
        error.context(BridgeError {
            code: "no_bridge".into(),
            reason: "session has no healthy bridged host".into(),
        })
    }
}

fn resolve(ctx: &Ctx, container: &ContainerRef) -> Result<Host> {
    let (binding, daemon_hosted) = match container {
        ContainerRef::Assistant { session } => {
            ensure!(
                ctx.store
                    .get_assistant(session)
                    .context("reading live assistant session")?
                    .ended
                    .is_none(),
                no_bridge()
            );
            let saved = ctx
                .store
                .get_assistant_container(session)
                .context("reading live assistant host")?;
            let daemon_hosted = saved
                .state
                .current_generation()
                .is_some_and(|g| g.daemon_hosted);
            (saved.state.binding, daemon_hosted)
        }
        ContainerRef::Pm => {
            let pm = pm_state::load(&ctx.store)
                .context("reading live PM host")?
                .ok_or_else(no_bridge)?;
            (pm.state.binding, false)
        }
        container => {
            let task = ctx
                .store
                .head(container.task().context("live task identity unavailable")?)
                .context("reading live session task")?;
            ensure!(
                agent::container_closure_ref(&task, container)
                    .context("checking live session lifecycle")?
                    .is_none(),
                no_bridge()
            );
            let host = task
                .agent_head(container)
                .context("reading live session binding")?
                .ok_or_else(no_bridge)?;
            (host.binding, false)
        }
    };
    ensure!(
        binding.health == HostHealth::Healthy && !binding.closing && binding.generation > 0,
        no_bridge()
    );
    let dir = if daemon_hosted {
        launch::assistant_runtime_dir(
            &ctx.store,
            container
                .assistant()
                .context("daemon host is not an assistant")?,
        )
        .join(format!("g{}", binding.generation))
    } else {
        let recipient = match container {
            ContainerRef::Pm => corral_core::inbox::Recipient::Pm,
            ContainerRef::Assistant { session } => corral_core::inbox::Recipient::Assistant {
                session: session.clone(),
            },
            container => crate::questions::recipient(container).ok_or_else(no_bridge)?,
        };
        launch::generation_dir(&ctx.store, &recipient, binding.generation)
            .context("resolving live session bridge directory")?
    };
    Ok(Host {
        generation: binding.generation,
        dir,
    })
}

fn validate(page: &LivePage, after: u64) -> Result<()> {
    ensure!(
        page.events.len() <= LIVE_HOST_EVENTS && page.next <= page.last,
        "invalid live page bounds"
    );
    ensure!(
        page.gap.as_ref().is_none_or(|gap| gap.reconcile),
        "invalid live gap"
    );
    let mut prior = page
        .gap
        .as_ref()
        .map_or(after, |gap| gap.resumed.saturating_sub(1));
    for event in &page.events {
        ensure!(
            event.generation == page.generation && event.seq > prior && event.seq <= page.next,
            "live events are not ordered within the generation"
        );
        ensure!(
            serde_json::to_vec(event)
                .context("sizing live event")?
                .len()
                <= LIVE_EVENT_BYTES,
            "live event exceeds byte bound"
        );
        let payload = match &event.kind {
            SessionLiveKind::LiveText { text, .. } | SessionLiveKind::LiveThinking { text, .. } => {
                Some(text)
            }
            SessionLiveKind::LiveTool {
                state: SessionLiveToolState::Started { args, .. },
                ..
            } => args.as_ref(),
            SessionLiveKind::LiveTool {
                state: SessionLiveToolState::Updated { partial },
                ..
            } => partial.as_ref(),
            SessionLiveKind::LiveTurn {
                state: SessionLiveTurnState::Ended { error, .. },
                ..
            } => {
                ensure!(
                    error
                        .as_ref()
                        .is_none_or(|text| text.len() <= LIVE_ERROR_BYTES),
                    "live error exceeds byte bound"
                );
                None
            }
            _ => None,
        };
        ensure!(
            payload.is_none_or(|text| text.len() <= LIVE_PAYLOAD_BYTES),
            "live payload exceeds byte bound"
        );
        prior = event.seq;
    }
    ensure!(page.next >= prior, "live cursor moved backwards");
    Ok(())
}

/// Keep complete newest messages first, then complete newest children within the page budget.
/// The total remains the full journal count even when retention or byte bounds remove entries.
pub fn bound_fleet(fleet: &mut SessionFleet, max_bytes: usize) -> Result<()> {
    fleet.messages_total = fleet
        .messages_total
        .max(fleet.messages.last().map_or(0, |message| message.seq));
    let excess = fleet.messages.len().saturating_sub(100);
    fleet.messages.drain(..excess);
    for message in &mut fleet.messages {
        if let Some((end, _)) = message.text.char_indices().nth(2048) {
            message.text.truncate(end);
        }
    }
    let mut bytes = serde_json::to_vec(fleet)
        .context("sizing session fleet")?
        .len();
    let mut remove = 0;
    for message in &fleet.messages {
        if bytes <= max_bytes {
            break;
        }
        let removed = serde_json::to_vec(message)
            .context("sizing agent message")?
            .len();
        bytes = bytes.saturating_sub(removed);
        remove += 1;
    }
    fleet.messages.drain(..remove);
    remove = 0;
    for child in &fleet.children {
        if bytes <= max_bytes {
            break;
        }
        let removed = serde_json::to_vec(child)
            .context("sizing session child")?
            .len();
        bytes = bytes.saturating_sub(removed);
        remove += 1;
    }
    fleet.children.drain(..remove);
    ensure!(bytes <= max_bytes, "session fleet exceeds byte budget");
    Ok(())
}

#[cfg(test)]
#[path = "session_live_tests.rs"]
mod tests;
