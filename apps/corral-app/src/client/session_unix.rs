use crate::client::core::{Core, Event};
use crate::client::transport;
use corral_protocol::frame::{read_frame_until, write_frame_until};
use corral_protocol::subscription::Topic;
use corral_protocol::{self as wire, Welcome};
use serde_json::Value;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const READ_SLICE: Duration = Duration::from_millis(250);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct Session {
    stream: UnixStream,
    core: Core,
    started: Instant,
    last_activity: Instant,
}

impl Session {
    pub(crate) fn connect(socket: &Path) -> Result<(Self, Welcome), String> {
        let started = Instant::now();
        let stream = transport::connect(socket, started + IO_TIMEOUT)?;
        let mut session = Self {
            stream,
            core: Core::new(),
            started,
            last_activity: Instant::now(),
        };
        session.core.hello(
            &[wire::BOARD_V1, corral_protocol::device::DEVICE_V1],
            session.now_ms(),
        )?;
        session.flush()?;
        loop {
            if let Some(welcome) = session.core.take_welcome() {
                return Ok((session, welcome));
            }
            session.check_deadlines()?;
            session.receive(Instant::now() + READ_SLICE)?;
        }
    }

    pub(crate) fn open(&mut self, topic: Topic) -> Result<(), String> {
        self.core.open(topic, self.now_ms())?;
        self.flush()
    }

    pub(crate) fn request(&mut self, method: &'static str, params: Value) -> Result<(), String> {
        self.core.request(method, params, self.now_ms())?;
        self.flush()
    }

    pub(crate) fn next(&mut self) -> Event {
        match self.next_event() {
            Ok(event) => event,
            Err(reason) => Event::Closed(one_line(reason)),
        }
    }

    fn next_event(&mut self) -> Result<Event, String> {
        let deadline = Instant::now() + READ_SLICE;
        loop {
            self.check_deadlines()?;
            if let Some(event) = self.core.next_event() {
                return Ok(event);
            }
            if self.last_activity.elapsed() >= IDLE_TIMEOUT {
                self.request(wire::PING, serde_json::json!({}))?;
                self.last_activity = Instant::now();
            }
            if Instant::now() >= deadline || !self.receive(deadline)? {
                self.check_deadlines()?;
                return Ok(Event::Idle);
            }
        }
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    fn check_deadlines(&self) -> Result<(), String> {
        if let Some(method) = self.core.expired(self.now_ms()) {
            return Err(format!("daemon {method} deadline expired"));
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        while let Some(frame) = self.core.take_outbox() {
            write_frame_until(&mut self.stream, &frame, Instant::now() + IO_TIMEOUT)
                .map_err(|error| format!("sending daemon frame: {error}"))?;
        }
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<bool, String> {
        if !transport::readable(&self.stream, deadline)? {
            return Ok(false);
        }
        // A quiet socket is not a framing error; a partially read frame is.
        let frame = read_frame_until(&mut self.stream, deadline)
            .map_err(|error| format!("reading daemon frame: {error}"))?
            .ok_or_else(|| "daemon closed the socket".to_owned())?;
        self.last_activity = Instant::now();
        self.core.receive(frame, self.now_ms())?;
        self.flush()?;
        Ok(true)
    }
}

fn one_line(text: String) -> String {
    if text.contains(['\r', '\n']) {
        text.replace(['\r', '\n'], " ")
    } else {
        text
    }
}
