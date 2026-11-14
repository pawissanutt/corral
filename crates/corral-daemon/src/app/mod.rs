//! Explicitly enabled, authenticated loopback app connections.
mod session;
mod ws;

use std::net::TcpListener;
use std::sync::Arc;

use anyhow::{Context, Result};
use corral_ops::Ctx;

use crate::{server, state::Shared};

const MAX_APP_CONNECTIONS: usize = 8;

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) {
    let Some(listen) = &ctx.store.config.daemon.listen else {
        return;
    };
    let origins: Vec<_> = ctx
        .store
        .config
        .daemon
        .web_origins
        .iter()
        .map(|origin| origin.as_str().to_owned())
        .collect();
    if origins.is_empty() {
        eprintln!(
            "corrald app listener: listening, but [daemon] web_origins is empty, so no browser can connect"
        );
    }
    let result = (|| -> Result<()> {
        let devices = shared.devices().context("device registry is unavailable")?;
        devices
            .identity()
            .map_err(|error| anyhow::anyhow!("{}", error.wire().1))?;
        let listener = TcpListener::bind(listen.addr()).context("binding app listener")?;
        serve_listener(listener, Arc::clone(shared), origins)
    })();
    if let Err(error) = result {
        eprintln!("corrald app listener: {error:#}");
    }
}

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
) -> Result<()> {
    listener
        .set_nonblocking(true)
        .context("setting app listener nonblocking")?;
    let port = listener
        .local_addr()
        .context("reading app listener address")?
        .port();
    let devices = shared.devices().context("device registry is unavailable")?;
    devices.set_port(port);
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
                    Ok((stream, _)) => {
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
                                if let Err(error) = session::serve(stream, shared, &origins) {
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
