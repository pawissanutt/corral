use super::{
    map,
    session::{Event, Session},
};
use crate::model::daemon::Feed;
use nsh_core::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn start(feed: Signal<Feed>, socket: Result<PathBuf, String>) {
    spawn(async move {
        feed.update(|feed| feed.host = hostname());
        let socket = match socket {
            Ok(socket) => socket,
            Err(problem) => {
                feed.update(|feed| {
                    tick(feed);
                    feed.link_lost(problem);
                });
                return;
            }
        };
        let mut failures = 0usize;
        loop {
            feed.update(|feed| {
                tick(feed);
                feed.connecting(socket.to_string_lossy().into_owned());
            });
            let path = socket.clone();
            let connected = match blocking(move || Session::connect(&path)).await {
                Ok(connected) => connected,
                Err(error) => Err(format!("daemon connection worker closed: {error}")),
            };
            match connected {
                Ok((mut session, welcome)) => {
                    feed.update(|feed| {
                        tick(feed);
                        feed.welcome_received(welcome.daemon.store, map::health(welcome.health));
                    });
                    loop {
                        let next = blocking(move || {
                            let event = session.next();
                            (session, event)
                        })
                        .await;
                        let (returned, event) = match next {
                            Ok(next) => next,
                            Err(error) => {
                                feed.update(|feed| {
                                    tick(feed);
                                    feed.link_lost(format!(
                                        "daemon session worker closed: {error}"
                                    ));
                                });
                                break;
                            }
                        };
                        session = returned;
                        let closed = matches!(&event, Event::Closed(_));
                        if matches!(&event, Event::Board { .. }) {
                            failures = 0;
                        }
                        feed.update(|feed| {
                            tick(feed);
                            match event {
                                Event::Health(health) => feed.health_received(map::health(health)),
                                Event::Board {
                                    epoch,
                                    rows,
                                    left,
                                    replacement,
                                } => {
                                    feed.board_received(
                                        epoch,
                                        rows.into_iter().map(map::task).collect(),
                                        &left,
                                        replacement,
                                    );
                                }
                                Event::Idle => {}
                                Event::Closed(problem) => feed.link_lost(problem),
                            }
                        });
                        if closed {
                            break;
                        }
                    }
                }
                Err(problem) => feed.update(|feed| {
                    tick(feed);
                    feed.link_lost(problem);
                }),
            }
            let delay = match failures {
                0 => 1,
                1 => 2,
                2 => 5,
                _ => 10,
            };
            failures = failures.saturating_add(1);
            timer(Duration::from_secs(delay)).await;
        }
    });
}

fn tick(feed: &mut Feed) {
    let local = jiff::Zoned::now();
    let seconds = match u64::try_from(local.timestamp().as_second()) {
        Ok(seconds) => seconds,
        Err(_) => 0,
    };
    feed.tick(
        seconds,
        format!("{:02}:{:02}", local.hour(), local.minute()),
    );
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let system = rustix::system::uname();
    let name = system.nodename().to_str().ok()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    None
}
