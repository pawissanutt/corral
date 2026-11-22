//! Deliver events to React extensions off the poll loop and every engine operation.
use std::{
    sync::{Arc, Weak},
    thread,
    time::Duration,
};

use corral_engine::{Config, Store, react};

use crate::state::Shared;

const PASS_INTERVAL: Duration = Duration::from_secs(1);
/// A failing event is retried on a later pass no sooner than this.
const RETRY_AFTER: Duration = Duration::from_secs(30);

// A line may run for its whole deadline. Hold no daemon reference while it runs and never
// join this thread on shutdown: an interrupted delivery leaves its cursor unadvanced, so the
// next daemon delivers that event again.
pub(crate) fn start(shared: &Arc<Shared>) {
    let root = shared.root().to_owned();
    let weak = Arc::downgrade(shared);
    if let Err(error) = thread::Builder::new()
        .name("corrald-react".into())
        .spawn(move || {
            let state_dir = root.join(crate::STATE_DIR);
            let store = match Store::open(&state_dir.join("corral.db")) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("corrald extension delivery unavailable: {error:#}");
                    return;
                }
            };
            let stopped = || stopped(&weak);
            while !stopped() {
                match Config::load(&state_dir.join("corral.toml")) {
                    Ok(config) => {
                        let reports = react::deliver(&react::Pass {
                            root: &root,
                            state_dir: &state_dir,
                            config: &config,
                            store: &store,
                            retry_after: RETRY_AFTER,
                            stop: &stopped,
                        });
                        for report in &reports {
                            if let Some(error) = &report.error {
                                eprintln!("corrald extension {}: {error}", report.extension);
                            }
                        }
                        if reports.iter().any(react::Report::changed)
                            && let Some(shared) = weak.upgrade()
                        {
                            shared.publish();
                        }
                    }
                    Err(error) => eprintln!("corrald extension delivery: {error:#}"),
                }
                let Some(shared) = weak.upgrade() else { break };
                shared.wait(PASS_INTERVAL);
            }
        })
    {
        eprintln!("corrald extension delivery unavailable: {error}");
    }
}

fn stopped(shared: &Weak<Shared>) -> bool {
    shared.upgrade().is_none_or(|shared| shared.lock().stop)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use corral_protocol::{
        rows::{ExtensionHealth, SettingsSnapshot},
        subscription::Topic,
    };

    use crate::views::{Rows, tests::Fixture};

    #[test]
    fn settings_list_each_extension_with_its_delivery_health() {
        let fixture = Fixture::new();
        let shared = &fixture.shared;
        let state = shared.root().join(crate::STATE_DIR);
        fs::write(
            state.join("corral.toml"),
            "[extensions.idle]\nenabled = false\nfacts = \"true\"\n\n\
             [extensions.notify]\non = [\"landed\"]\nrun = \"exit 3\"\n\
             [extensions.notify.tools]\nping = { run = \"true\" }\n",
        )
        .unwrap();
        fs::create_dir_all(state.join("extensions")).unwrap();
        fs::write(
            state.join("extensions/notify.json"),
            r#"{"seq":7,"skipped":2,"last_error":"event 7: exit 3"}"#,
        )
        .unwrap();
        let rows = shared.row_cache.get(shared, Topic::Settings).unwrap();
        let Rows::Settings(rows) = rows.as_ref() else {
            panic!("expected Settings rows");
        };
        let SettingsSnapshot { extensions, .. } = &rows[0];
        assert_eq!(
            extensions,
            &[
                ExtensionHealth {
                    name: "idle".into(),
                    shapes: vec!["ask".into()],
                    enabled: false,
                    last_error: None,
                    skipped: 0,
                },
                ExtensionHealth {
                    name: "notify".into(),
                    shapes: vec!["tool".into(), "react".into()],
                    enabled: true,
                    last_error: Some("event 7: exit 3".into()),
                    skipped: 2,
                },
            ]
        );
    }
}
