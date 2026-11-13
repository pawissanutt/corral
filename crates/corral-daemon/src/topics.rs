//! Per-connection cursors and one in-flight marker; no per-client row queue.
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, bail, ensure};
use corral_ops::boardview::DeltaOutcome;
use corral_protocol::{
    self as wire,
    board::BoardCursor,
    frame::{CONTROL_MAX, Frame, FrameKind, write_frame_until},
    rpc,
    subscription::{SubAck, SubClose, SubDelta, SubOpen, SubReset, Topic},
};
use serde::Serialize;
use std::os::unix::net::UnixStream;

use crate::state::Shared;

struct Subscriber {
    epoch: u64,
    cursor: BoardCursor,
    in_flight: bool,
    initial: bool,
}

pub(super) struct Topics<'a> {
    shared: &'a Shared,
    board: Option<Subscriber>,
    health: Option<Subscriber>,
}

impl<'a> Topics<'a> {
    pub fn new(shared: &'a Shared) -> Self {
        Self {
            shared,
            board: None,
            health: None,
        }
    }

    pub fn open(&mut self, open: SubOpen) -> Result<()> {
        ensure!(open.topic != Topic::Other, "unsupported topic");
        self.close(SubClose { topic: open.topic });
        let epoch = self.shared.epoch();
        let (epoch, cursor) = open.since.map_or((epoch, BoardCursor::default()), |since| {
            (since.epoch, since.cursor)
        });
        let subscriber = Some(Subscriber {
            epoch,
            cursor,
            in_flight: false,
            initial: true,
        });
        match open.topic {
            Topic::Board => {
                self.board = subscriber;
                self.shared
                    .board_subscribers
                    .fetch_add(1, Ordering::Relaxed);
                let mut state = self.shared.lock();
                state.projection_wake = state.projection_wake.wrapping_add(1);
                self.shared.changed.notify_all();
            }
            Topic::Health => self.health = subscriber,
            Topic::Classic | Topic::Other => bail!("unsupported topic"),
        }
        Ok(())
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        let subscriber = match ack.topic {
            Topic::Board => self.board.as_mut(),
            Topic::Health => self.health.as_mut(),
            Topic::Classic | Topic::Other => None,
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
            Topic::Classic | Topic::Other => {}
        }
    }

    pub fn flush(&mut self, stream: &mut UnixStream) -> Result<()> {
        for topic in [Topic::Board, Topic::Health] {
            let result = self.next(topic);
            match result {
                Ok(Some(frame)) => write_frame_until(
                    stream,
                    &frame,
                    std::time::Instant::now() + super::IO_TIMEOUT,
                )
                .context("pushing topic frame")?,
                Ok(None) => {}
                Err(error) => {
                    super::error(
                        stream,
                        serde_json::Value::Null,
                        rpc::ErrorCode::Internal,
                        &format!("{error:#}"),
                    )?;
                    self.close(SubClose { topic });
                }
            }
        }
        Ok(())
    }

    fn next(&mut self, topic: Topic) -> Result<Option<Frame>> {
        let subscriber = match topic {
            Topic::Board => self.board.as_mut(),
            Topic::Health => self.health.as_mut(),
            Topic::Classic | Topic::Other => None,
        };
        let Some(subscriber) = subscriber.filter(|subscriber| !subscriber.in_flight) else {
            return Ok(None);
        };
        let epoch = self.shared.epoch();
        if subscriber.epoch != epoch {
            let frame = notification(wire::SUB_RESET, SubReset { topic, epoch })?;
            self.close(SubClose { topic });
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
                            let frame = notification(wire::SUB_RESET, SubReset { topic, epoch })?;
                            self.close(SubClose { topic });
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
            Topic::Classic | Topic::Other => bail!("unsupported topic"),
        }
    }
}

impl Drop for Topics<'_> {
    fn drop(&mut self) {
        self.close(SubClose {
            topic: Topic::Board,
        });
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
