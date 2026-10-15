//! Harness discovery off the UI thread. Probes run subprocesses with multi-second timeouts, so screens (the
//! settings Harnesses/Profiles section, the dispatch sheet) start a `HarnessLoader` and poll it each frame,
//! the same way the app drains `WorkerMsg` from its sync worker.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use corral_ops::{Ctx, HarnessReport};

/// What the discovery worker sends: the report, or the error text for a toast.
pub type HarnessMsg = Result<HarnessReport, String>;

/// One background discovery run; `start` again (with `refresh`) for the `r` key.
pub struct HarnessLoader {
    rx: Option<Receiver<HarnessMsg>>,
}

impl HarnessLoader {
    /// Spawns the worker: cached report unless `refresh`, which re-detects.
    pub fn start(ctx: &Ctx, refresh: bool) -> HarnessLoader {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = corral_ops::harness_report(&ctx, refresh).map_err(|e| format!("{e:#}"));
            let _ = tx.send(msg);
        });
        HarnessLoader { rx: Some(rx) }
    }

    /// Non-blocking: the result once it has arrived (returned exactly once), else `None`.
    pub fn poll(&mut self) -> Option<HarnessMsg> {
        let msg = match self.rx.as_ref()?.try_recv() {
            Ok(msg) => msg,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("harness discovery worker exited".into()),
        };
        self.rx = None;
        Some(msg)
    }

    /// True until `poll` has returned the result.
    pub fn pending(&self) -> bool {
        self.rx.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use corral_core::{Harness, Store};

    use super::*;

    #[test]
    fn delivers_the_report_once() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut store = Store::init(tmp.path()).expect("init");
        store.config.harnesses.clear();
        store.config.agents.clear();
        store.config.harnesses.insert(
            "claude".into(),
            Harness {
                command: "/nonexistent/corral-test/claude {model_args}".into(),
                flags: None,
                model_flag: None,
                effort_flag: None,
                efforts: Vec::new(),
                prompt: None,
            },
        );
        let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
        let mut loader = HarnessLoader::start(&ctx, true);
        let rx = loader.rx.take().expect("receiver");
        let msg = rx
            // Report delivery is the signal; allow a loaded host to schedule the worker.
            .recv_timeout(Duration::from_secs(120))
            .expect("worker answers");
        let report = msg.expect("report");
        let claude = report.get("claude").expect("claude");
        assert!(!claude.installed());
        assert_eq!(claude.models, ["opus", "sonnet", "haiku"]);
        assert!(!loader.pending());
        assert!(loader.poll().is_none());
    }
}
