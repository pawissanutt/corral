//! `classic.v1`: change notices for the classic board frame. A notice only says that the
//! daemon's frame moved; the board then fetches the document as a served read
//! (`ReadKind::ClassicFrame`). One notice is in flight until it is acknowledged.
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::Store;
use corral_protocol::board::BoardCursor;
use corral_protocol::classic::ClassicNotice;
use corral_protocol::frame::{FrameKind, read_frame_until};
use corral_protocol::rpc::{JSONRPC, Notification};
use corral_protocol::subscription::{SubAck, SubDelta, SubOpen, Topic};

use super::{Client, IO_TIMEOUT};

/// An open `classic` topic subscription on its own connection.
pub struct ClassicWatch {
    client: Client,
    epoch: Option<u64>,
    rev: u64,
}

impl ClassicWatch {
    /// None: no daemon, or it lacks `classic.v1`.
    pub fn open(store: &Store) -> Result<Option<Self>> {
        let Some(mut client) = Client::connect(store)? else {
            return Ok(None);
        };
        if !client.classic_capable {
            return Ok(None);
        }
        let _: serde_json::Value = client
            .request(
                corral_protocol::SUB_OPEN,
                SubOpen {
                    topic: Topic::Classic,
                    since: None,
                },
            )
            .context("opening the classic subscription")?;
        Ok(Some(Self {
            client,
            epoch: None,
            rev: 0,
        }))
    }

    /// Waits up to `timeout` for the next notice and acks it. Ok(None) on timeout.
    /// Err on reset, close or protocol error (the caller reopens).
    pub fn next(&mut self, timeout: Duration) -> Result<Option<ClassicNotice>> {
        if !self.readable(Instant::now() + timeout)? {
            return Ok(None);
        }
        // The first byte is here: the rest of the frame gets the ordinary I/O bound.
        let frame = read_frame_until(&mut self.client.stream, Instant::now() + IO_TIMEOUT)
            .context("reading classic notice")?
            .context("daemon closed the classic subscription")?;
        if frame.kind != FrameKind::Control || frame.stream != 0 {
            bail!("daemon returned a non-control classic frame");
        }
        let notification: Notification =
            serde_json::from_slice(&frame.payload).context("decoding classic notification")?;
        if notification.jsonrpc != JSONRPC {
            bail!("invalid classic notification version");
        }
        match notification.method.as_str() {
            corral_protocol::SUB_DELTA => {}
            corral_protocol::SUB_RESET => bail!("daemon reset the classic subscription"),
            _ => bail!("daemon closed or failed the classic subscription"),
        }
        let delta: SubDelta =
            serde_json::from_value(notification.params).context("decoding classic notice")?;
        let SubDelta::Classic { notice } = delta else {
            bail!("daemon returned the wrong subscription topic");
        };
        if self.epoch.is_some_and(|epoch| epoch != notice.epoch) {
            bail!("classic epoch changed without sub.reset");
        }
        if self.epoch.is_some() && notice.rev <= self.rev {
            bail!("classic revision did not advance");
        }
        let _: serde_json::Value = self.client.request(
            corral_protocol::SUB_ACK,
            SubAck {
                topic: Topic::Classic,
                cursor: BoardCursor {
                    rev: notice.rev,
                    after: None,
                },
            },
        )?;
        self.epoch = Some(notice.epoch);
        self.rev = notice.rev;
        Ok(Some(notice))
    }

    /// Whether a frame starts before `deadline`. Polling first keeps an idle timeout from
    /// leaving a partly read frame on the connection.
    fn readable(&self, deadline: Instant) -> Result<bool> {
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(false);
            };
            let timeout = rustix::event::Timespec::try_from(remaining)
                .context("classic wait exceeds the poll range")?;
            match rustix::event::poll(
                &mut [rustix::event::PollFd::new(
                    &self.client.stream,
                    rustix::event::PollFlags::IN,
                )],
                Some(&timeout),
            ) {
                Ok(0) => return Ok(false),
                Ok(_) => return Ok(true),
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error).context("waiting for a classic notice"),
            }
        }
    }
}
