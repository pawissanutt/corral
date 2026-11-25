//! What a sample pi host does when its session is controlled: Cancel, Pause, Resume and the
//! rewinds, which save the work under a rescue ref and cut the transcript back. The streaming
//! and delivery they act on are in `pi.rs`; the cards the host waits on are answered in
//! `build_ops.rs`.

use corral_protocol::session::{RewindReply, Rewound};
use corral_protocol::subscription::{SessionLiveKind, SessionLiveTurnState};
use corral_types::Origin;
use serde_json::{Value, json};

use super::pi::{Input, finish, publish, start};
use super::pi_rows::settle;
use super::sample::sha;
use super::stamp;
use crate::model::daemon::Feed;

/// A session control a pi host answers: Cancel, Pause, Resume and the rewinds. The reply is
/// corrald's; `Err` is its refusal.
pub(super) fn control(
    feed: &mut Feed,
    id: &str,
    control: crate::model::agents::Control,
) -> Result<Value, String> {
    use crate::model::agents::Control;
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get(id)
        .ok_or_else(|| "session has no running Pi host; relaunch it first".to_owned())?;
    if host.gone {
        return Err("session has ended; relaunch it before sending controls".into());
    }
    let list = RewindReply::List {
        checkpoints: host.checkpoints.clone(),
        rewinds: host.rewinds.clone(),
    };
    let reply = match control {
        Control::Cancel => {
            let turn = cancel(feed, id, &at);
            match turn {
                Some(turn) => json!({"cancelled": true, "turn": turn}),
                None => json!({"cancelled": false, "reason": "no turn is running"}),
            }
        }
        Control::Pause => {
            let turn = cancel(feed, id, &at);
            if let Some(host) = feed.hosts.get_mut(id) {
                host.paused = true;
            }
            json!({"paused": true, "cancelled": turn.is_some(), "turn": turn})
        }
        Control::Resume => {
            let held = feed
                .hosts
                .get_mut(id)
                .map(|host| {
                    host.paused = false;
                    std::mem::take(&mut host.held)
                })
                .unwrap_or_default();
            if !held.is_empty() {
                let inputs = held
                    .into_iter()
                    .map(|text| Input {
                        text,
                        origin: Origin::Auto,
                        entry: None,
                    })
                    .collect();
                start(feed, id, inputs);
            }
            json!({"paused": false})
        }
        Control::Rewinds => serde_json::to_value(list).map_err(|error| error.to_string())?,
        Control::Rewind(turn) => rewind(feed, id, turn, &at)?,
        Control::Terminate | Control::Relaunch | Control::ResumeCode => {
            return Err("terminate and relaunch go through corrald, not the host".into());
        }
    };
    settle(feed, id);
    Ok(reply)
}

/// Cancels the running turn, keeping what it said; returns its number.
pub(super) fn cancel(feed: &mut Feed, id: &str, at: &str) -> Option<u64> {
    let host = feed.hosts.get_mut(id)?;
    let turn = host.open.take()?;
    finish(host, at, &turn, None, true);
    let ended = SessionLiveKind::LiveTurn {
        turn: turn.turn,
        state: SessionLiveTurnState::Ended {
            aborted: true,
            error: None,
        },
    };
    let event = host.event(at, ended);
    publish(feed, id, vec![event], false);
    Some(turn.turn)
}

/// Saves the work under a rescue ref, then cuts the transcript back to the start of `turn`. The
/// live stream starts a new generation, so every watcher re-reads the transcript.
fn rewind(feed: &mut Feed, id: &str, turn: u64, at: &str) -> Result<Value, String> {
    let point = feed
        .hosts
        .get(id)
        .and_then(|host| host.checkpoints.iter().find(|point| point.turn == turn))
        .cloned()
        .ok_or_else(|| format!("no checkpoint at the start of turn {turn}"))?;
    cancel(feed, id, at);
    let host = feed
        .hosts
        .get_mut(id)
        .ok_or_else(|| "session has no running Pi host".to_owned())?;
    let cut = host
        .starts
        .get(
            usize::try_from(turn)
                .unwrap_or(usize::MAX)
                .saturating_sub(1),
        )
        .copied()
        .unwrap_or(host.transcript.len());
    let number = host.rewinds.len() + 1;
    let rewound = Rewound {
        turn,
        checkpoint: point.id.clone(),
        rescue_ref: format!("refs/worktree/corral/rescue/{id}/{number}"),
        from_head: sha(&format!("{id}/rescue/{number}"), 40),
        head: point.head.clone().unwrap_or_default(),
        at: Some(at.into()),
    };
    host.transcript.truncate(cut);
    host.starts.truncate(cut_turns(turn));
    host.checkpoints.retain(|point| point.turn < turn);
    host.turn = turn.saturating_sub(1);
    host.pending.clear();
    host.queued.clear();
    host.rewinds.push(rewound.clone());
    host.generation += 1;
    publish(feed, id, Vec::new(), true);
    serde_json::to_value(RewindReply::Turn(rewound)).map_err(|error| error.to_string())
}

fn cut_turns(turn: u64) -> usize {
    usize::try_from(turn.saturating_sub(1)).unwrap_or(usize::MAX)
}
