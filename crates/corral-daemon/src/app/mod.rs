//! Explicitly enabled, authenticated app connections; remote browsers use a TLS proxy.
pub(crate) mod dispatch;
mod session;
mod ws;

use std::net::TcpListener;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::{server, state::Shared};

const MAX_APP_CONNECTIONS: usize = 8;

struct AppPermit(Arc<Shared>);
impl Drop for AppPermit {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.workers -= 1;
        state.app_workers -= 1;
        self.0.changed.notify_all();
    }
}

pub(crate) fn serve_listener(
    listener: TcpListener,
    shared: Arc<Shared>,
    origins: Vec<String>,
    endpoint: Option<&str>,
) -> Result<()> {
    listener
        .set_nonblocking(true)
        .context("setting app listener nonblocking")?;
    let address = listener
        .local_addr()
        .context("reading app listener address")?;
    shared.devices().set_listener(address, &origins, endpoint);
    shared.publish();
    std::thread::Builder::new()
        .name("corrald-app-server".into())
        .spawn(move || {
            while !shared.lock().stop {
                match rustix::event::poll(
                    &mut [rustix::event::PollFd::new(
                        &listener,
                        rustix::event::PollFlags::IN,
                    )],
                    Some(&rustix::event::Timespec {
                        tv_sec: 0,
                        tv_nsec: 500_000_000,
                    }),
                ) {
                    Ok(0) => continue,
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(error) => {
                        eprintln!("corrald app listener: polling: {error}");
                        break;
                    }
                }
                if shared.lock().stop {
                    break;
                }
                match listener.accept() {
                    Ok((stream, peer)) => {
                        {
                            let mut state = shared.lock();
                            if state.stop
                                || state.workers >= server::MAX_CONNECTIONS
                                || state.app_workers >= MAX_APP_CONNECTIONS
                            {
                                continue;
                            }
                            state.workers += 1;
                            state.app_workers += 1;
                        }
                        let permit = AppPermit(Arc::clone(&shared));
                        let shared = Arc::clone(&shared);
                        let origins = origins.clone();
                        if let Err(error) = std::thread::Builder::new()
                            .name("corrald-app-client".into())
                            .spawn(move || {
                                let _permit = permit;
                                if let Err(error) = session::serve(stream, peer, shared, &origins) {
                                    eprintln!("corrald app client: {error:#}");
                                }
                            })
                        {
                            eprintln!("corrald app accepting client: {error:#}");
                        }
                    }
                    Err(error) => server::accept_error(&shared, error),
                }
            }
        })
        .context("starting app listener")?;
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
