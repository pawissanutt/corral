//! Explicitly enabled, authenticated app connections; remote browsers use a TLS proxy.
pub(crate) mod dispatch;
mod http;
mod session;
mod static_site;
mod ws;

use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};

use crate::{server, state::Shared};

const MAX_APP_CONNECTIONS: usize = 8;
// NSH starts more than a dozen asset fetches at once. HTTP/2 frontends
// can forward that burst concurrently even though the upstream is HTTP/1.1.
const MAX_PENDING_HEADERS: usize = 32;
const MAX_STATIC_TRANSFERS: usize = 32;

struct StaticHost {
    root: PathBuf,
    pending: Arc<AtomicUsize>,
    transfers: Arc<AtomicUsize>,
}

struct HttpPermit(Arc<AtomicUsize>);

impl HttpPermit {
    fn acquire(counter: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < limit).then_some(count + 1)
            })
            .ok()?;
        Some(Self(Arc::clone(counter)))
    }
}

impl Drop for HttpPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

struct AppPermit(Arc<Shared>);

impl AppPermit {
    fn acquire(shared: &Arc<Shared>) -> Option<Self> {
        let mut state = shared.lock();
        if state.stop
            || state.workers >= server::MAX_CONNECTIONS
            || state.app_workers >= MAX_APP_CONNECTIONS
        {
            return None;
        }
        state.workers += 1;
        state.app_workers += 1;
        Some(Self(Arc::clone(shared)))
    }
}
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
    web_dir: Option<PathBuf>,
) -> Result<()> {
    listener
        .set_nonblocking(true)
        .context("setting app listener nonblocking")?;
    let address = listener
        .local_addr()
        .context("reading app listener address")?;
    shared.devices().set_listener(address, &origins, endpoint);
    shared.publish();
    let origins: Arc<[String]> = origins.into();
    let host = web_dir.map(|root| {
        Arc::new(StaticHost {
            root,
            pending: Arc::default(),
            transfers: Arc::default(),
        })
    });
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
                    Ok((stream, peer)) => accept(stream, peer, &shared, &origins, host.as_ref()),
                    Err(error) => server::accept_error(&shared, error),
                }
            }
        })
        .context("starting app listener")?;
    Ok(())
}

fn accept(
    stream: TcpStream,
    peer: SocketAddr,
    shared: &Arc<Shared>,
    origins: &Arc<[String]>,
    host: Option<&Arc<StaticHost>>,
) {
    let stream = ws::Bounded::incoming(stream);
    if let Some(host) = host {
        let Some(pending) = HttpPermit::acquire(&host.pending, MAX_PENDING_HEADERS) else {
            return;
        };
        let (shared, origins, host) = (Arc::clone(shared), Arc::clone(origins), Arc::clone(host));
        spawn_client(move || {
            http::route(stream, &host, pending, |stream, pending| {
                let permit = AppPermit::acquire(&shared);
                drop(pending);
                if let Some(_permit) = permit {
                    report(session::serve(stream, peer, shared, &origins));
                }
            })
        });
    } else if let Some(permit) = AppPermit::acquire(shared) {
        let (shared, origins) = (Arc::clone(shared), Arc::clone(origins));
        spawn_client(move || {
            let _permit = permit;
            report(session::serve(stream, peer, shared, &origins));
        });
    }
}

// Called by the worker while it still owns its lane's permit. Even a blocked
// log sink must not let failed requests accumulate uncounted threads.
fn report(result: Result<()>) {
    if let Err(error) = result {
        eprintln!("corrald app client: {error:#}");
    }
}

fn spawn_client(serve: impl FnOnce() + Send + 'static) {
    if let Err(error) = std::thread::Builder::new()
        .name("corrald-app-client".into())
        .spawn(serve)
    {
        eprintln!("corrald app accepting client: {error:#}");
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
