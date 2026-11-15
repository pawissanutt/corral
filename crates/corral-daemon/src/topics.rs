//! Per-connection cursors and one in-flight marker; no per-client row queue.
mod app;
pub(crate) use app::session::{
    Service as SessionLiveService, join as join_session_live, start as start_session_live,
};
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, bail, ensure};
use corral_ops::boardview::DeltaOutcome;
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX as CONTROL_MAX,
    board::BoardCursor,
    classic::ClassicNotice,
    frame::{Frame, FrameKind},
    rpc,
    subscription::{SubAck, SubClose, SubDelta, SubOpen, SubReset, SubSince, Topic},
};
use serde::Serialize;

use crate::conn::Conn;
use crate::state::Shared;

struct Subscriber {
    epoch: u64,
    cursor: BoardCursor,
    in_flight: bool,
    initial: bool,
}

/// A `classic` subscription: the frame series it follows and the last revision noticed.
struct ClassicSubscriber {
    /// None until the first notice when the client opened without `since`.
    epoch: Option<u64>,
    rev: u64,
    in_flight: bool,
    initial: bool,
}

pub(super) struct Topics<'a> {
    shared: &'a Shared,
    board: Option<Subscriber>,
    health: Option<Subscriber>,
    classic: Option<ClassicSubscriber>,
    app: app::AppTopics,
}

impl<'a> Topics<'a> {
    pub fn new(shared: &'a Shared) -> Self {
        Self {
            shared,
            board: None,
            health: None,
            classic: None,
            app: app::AppTopics::default(),
        }
    }

    pub fn open(&mut self, open: SubOpen) -> Result<()> {
        ensure!(open.topic != Topic::Other, "unsupported topic");
        if !matches!(open.topic, Topic::Board | Topic::Health | Topic::Classic) {
            return self.app.open(self.shared, open);
        }
        self.close(SubClose {
            topic: open.topic,
            scope: None,
        });
        match open.topic {
            Topic::Board => {
                self.board = Some(subscriber(self.shared, open.since));
                self.shared
                    .board_subscribers
                    .fetch_add(1, Ordering::Relaxed);
                let mut state = self.shared.lock();
                state.projection_wake = state.projection_wake.wrapping_add(1);
                self.shared.changed.notify_all();
            }
            Topic::Health => self.health = Some(subscriber(self.shared, open.since)),
            Topic::Classic => {
                let (epoch, rev) = open
                    .since
                    .map_or((None, 0), |since| (Some(since.epoch), since.cursor.rev));
                self.classic = Some(ClassicSubscriber {
                    epoch,
                    rev,
                    in_flight: false,
                    initial: true,
                });
                // The reader thread starts refreshing the frame on its next idle wake.
                self.shared
                    .classic_subscribers
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => bail!("unsupported topic"),
        }
        Ok(())
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        let subscriber = match ack.topic {
            Topic::Board => self.board.as_mut(),
            Topic::Health => self.health.as_mut(),
            Topic::Classic => {
                let classic = self.classic.as_mut().context("topic is not open")?;
                ensure!(
                    classic.in_flight
                        && ack.cursor.rev == classic.rev
                        && ack.cursor.after.is_none(),
                    "ack does not match the in-flight cursor"
                );
                classic.in_flight = false;
                return Ok(());
            }
            Topic::Other => None,
            _ => return self.app.ack(ack),
        }
        .context("topic is not open")?;
        ensure!(
            subscriber.in_flight && subscriber.cursor == ack.cursor,
            "ack does not match the in-flight cursor"
        );
        subscriber.in_flight = false;
        Ok(())
    }

    pub fn close(&mut self, close: SubClose) {
        match close.topic {
            Topic::Board => {
                if self.board.take().is_some() {
                    self.shared
                        .board_subscribers
                        .fetch_sub(1, Ordering::Relaxed);
                }
            }
            Topic::Health => self.health = None,
            Topic::Classic => {
                if self.classic.take().is_some() {
                    self.shared
                        .classic_subscribers
                        .fetch_sub(1, Ordering::Relaxed);
                }
            }
            Topic::Other => {}
            _ => self.app.close(close.topic, close.scope),
        }
    }

    pub fn flush(&mut self, conn: &mut dyn Conn) -> Result<()> {
        for topic in [Topic::Board, Topic::Health, Topic::Classic] {
            let result = self.next(topic);
            match result {
                Ok(Some(frame)) => conn.send_frame(&frame).context("pushing topic frame")?,
                Ok(None) => {}
                Err(error) => {
                    crate::conn::error(
                        conn,
                        serde_json::Value::Null,
                        rpc::ErrorCode::Internal,
                        &format!("{error:#}"),
                    )?;
                    self.close(SubClose { topic, scope: None });
                }
            }
        }
        self.app.flush(self.shared, conn)
    }

    fn next(&mut self, topic: Topic) -> Result<Option<Frame>> {
        let subscriber = match topic {
            Topic::Board => self.board.as_mut(),
            Topic::Health => self.health.as_mut(),
            Topic::Classic => return self.next_classic(),
            _ => None,
        };
        let Some(subscriber) = subscriber.filter(|subscriber| !subscriber.in_flight) else {
            return Ok(None);
        };
        let epoch = self.shared.epoch();
        if subscriber.epoch != epoch {
            let frame = notification(
                wire::SUB_RESET,
                SubReset {
                    topic,
                    epoch,
                    scope: None,
                },
            )?;
            self.close(SubClose { topic, scope: None });
            return Ok(Some(frame));
        }
        match topic {
            Topic::Board => {
                let mut limit = 100;
                loop {
                    let outcome = self
                        .shared
                        .projection()
                        .as_ref()
                        .context("daemon projection is unavailable")?
                        .delta_since(subscriber.epoch, &subscriber.cursor, limit);
                    match outcome {
                        DeltaOutcome::Reset { epoch } => {
                            let frame = notification(
                                wire::SUB_RESET,
                                SubReset {
                                    topic,
                                    epoch,
                                    scope: None,
                                },
                            )?;
                            self.close(SubClose { topic, scope: None });
                            return Ok(Some(frame));
                        }
                        DeltaOutcome::Delta(delta) => {
                            if !subscriber.initial && delta.cursor == subscriber.cursor {
                                return Ok(None);
                            }
                            let cursor = delta.cursor.clone();
                            let frame = notification(wire::SUB_DELTA, SubDelta::Board { delta })?;
                            if frame.payload.len() <= CONTROL_MAX {
                                subscriber.cursor = cursor;
                                subscriber.initial = false;
                                subscriber.in_flight = true;
                                return Ok(Some(frame));
                            }
                            ensure!(
                                limit > 1,
                                "internal: a single board row exceeds CONTROL_MAX"
                            );
                            limit = (limit / 2).max(1);
                        }
                    }
                }
            }
            Topic::Health => {
                let mut health = self
                    .shared
                    .health_topic
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let snapshot = self.shared.health();
                if health.1.daemon != snapshot.daemon
                    || health.1.runtimes != snapshot.runtimes
                    || health.1.pm != snapshot.pm
                    || health.1.loop_facts.pass_seq != snapshot.loop_facts.pass_seq
                    || !health
                        .1
                        .reasons
                        .iter()
                        .map(|reason| crate::health::reason_key(reason))
                        .eq(snapshot
                            .reasons
                            .iter()
                            .map(|reason| crate::health::reason_key(reason)))
                {
                    health.0 = health
                        .0
                        .checked_add(1)
                        .context("health revision exhausted")?;
                }
                health.1 = snapshot;
                if !subscriber.initial && subscriber.cursor.rev == health.0 {
                    return Ok(None);
                }
                let cursor = BoardCursor {
                    rev: health.0,
                    after: None,
                };
                let delta = SubDelta::Health {
                    epoch,
                    rows: vec![health.1.clone()],
                    left: Vec::new(),
                    cursor: cursor.clone(),
                    more: false,
                };
                drop(health);
                let frame = notification(wire::SUB_DELTA, delta)?;
                ensure!(
                    frame.payload.len() <= CONTROL_MAX,
                    "internal: health snapshot exceeds CONTROL_MAX"
                );
                subscriber.cursor = cursor;
                subscriber.initial = false;
                subscriber.in_flight = true;
                Ok(Some(frame))
            }
            _ => bail!("unsupported topic"),
        }
    }

    /// One notice in flight; the latest published revision wins, older ones are never queued.
    fn next_classic(&mut self) -> Result<Option<Frame>> {
        let Some(classic) = self.classic.as_mut().filter(|classic| !classic.in_flight) else {
            return Ok(None);
        };
        let Some(published) = self
            .shared
            .classic
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        else {
            return Ok(None);
        };
        if classic.epoch.is_some_and(|epoch| epoch != published.epoch) {
            let topic = Topic::Classic;
            let frame = notification(
                wire::SUB_RESET,
                SubReset {
                    topic,
                    epoch: published.epoch,
                    scope: None,
                },
            )?;
            self.close(SubClose { topic, scope: None });
            return Ok(Some(frame));
        }
        if !classic.initial && published.rev <= classic.rev {
            return Ok(None);
        }
        let frame = notification(
            wire::SUB_DELTA,
            SubDelta::Classic {
                notice: ClassicNotice {
                    epoch: published.epoch,
                    rev: published.rev,
                    seq: published.seq,
                    as_of: published.as_of.clone(),
                },
            },
        )?;
        ensure!(
            frame.payload.len() <= CONTROL_MAX,
            "internal: classic notice exceeds CONTROL_MAX"
        );
        classic.epoch = Some(published.epoch);
        classic.rev = published.rev;
        classic.initial = false;
        classic.in_flight = true;
        Ok(Some(frame))
    }
}

fn subscriber(shared: &Shared, since: Option<SubSince>) -> Subscriber {
    let (epoch, cursor) = since.map_or_else(
        || (shared.epoch(), BoardCursor::default()),
        |since| (since.epoch, since.cursor),
    );
    Subscriber {
        epoch,
        cursor,
        in_flight: false,
        initial: true,
    }
}

impl Drop for Topics<'_> {
    fn drop(&mut self) {
        for topic in [Topic::Board, Topic::Classic] {
            self.close(SubClose { topic, scope: None });
        }
    }
}

fn notification(method: &str, params: impl Serialize) -> Result<Frame> {
    let payload = serde_json::to_vec(&rpc::Notification {
        jsonrpc: rpc::JSONRPC.into(),
        method: method.into(),
        params,
    })
    .context("encoding subscription notification")?;
    Ok(Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload,
    })
}
