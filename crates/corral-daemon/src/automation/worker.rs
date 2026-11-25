//! Keep host startup and rule evaluation off the daemon's poll and React loops.
use std::{sync::Arc, thread, time::Duration};

use corral_engine::{Config, Store};
use jiff::Timestamp;

use super::{Delivery, pass};
use crate::{assistant::automation as chat, state::Shared};

pub(crate) fn start(shared: &Arc<Shared>) {
    let state_dir = shared.root().join(crate::STATE_DIR);
    let weak = Arc::downgrade(shared);
    if let Err(error) = thread::Builder::new()
        .name("corrald-automation".into())
        .spawn(move || {
            let store = match Store::open(&state_dir.join("corral.db")) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("corrald automation unavailable: {error:#}");
                    return;
                }
            };
            let mut previous_error = None;
            while let Some(shared) = weak.upgrade() {
                if shared.lock().stop {
                    break;
                }
                let result = Config::load(&state_dir.join("corral.toml")).and_then(|config| {
                    pass(
                        &state_dir,
                        &store,
                        &config,
                        Timestamp::now(),
                        &mut |rule, profile, prior, id, text| {
                            if shared.lock().stop {
                                return Delivery {
                                    outcome: "skipped".into(),
                                    error: Some("daemon stopping".into()),
                                    chat: prior.map(str::to_owned),
                                };
                            }
                            match chat::deliver(&shared, rule, profile, prior, id, text) {
                                Ok(chat) => Delivery {
                                    outcome: "sent".into(),
                                    error: None,
                                    chat: Some(chat),
                                },
                                Err(error) => {
                                    let outcome = if error.is::<chat::Skip>() {
                                        "skipped"
                                    } else {
                                        "failed"
                                    };
                                    Delivery {
                                        outcome: outcome.into(),
                                        error: Some(format!("{error:#}")),
                                        chat: chat::allocated_chat(&shared, rule)
                                            .ok()
                                            .flatten()
                                            .or_else(|| prior.map(str::to_owned)),
                                    }
                                }
                            }
                        },
                    )
                });
                match result {
                    Ok(changed) => {
                        previous_error = None;
                        if changed {
                            shared.publish();
                        }
                    }
                    Err(error) => {
                        let error = format!("{error:#}");
                        if previous_error.as_ref() != Some(&error) {
                            eprintln!("corrald automation: {error}");
                            previous_error = Some(error);
                        }
                    }
                }
                shared.wait(Duration::from_secs(1));
            }
        })
    {
        eprintln!("corrald automation unavailable: {error:#}");
    }
}
