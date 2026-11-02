//! Independent source/canonical-document cost model for the whole app's root sync pass.
use corral_core::{
    Store,
    work::{Counter, WorkSnapshot},
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn documents(store: &Store, assistant: &str) -> Vec<serde_json::Value> {
    [
        ("agent-pm", "current"),
        ("pm-inbox", "current"),
        ("assistant-sessions", assistant),
        ("assistant-containers", assistant),
    ]
    .map(|(area, key)| store.record_row(area, key).unwrap().unwrap().doc)
    .into()
}

fn file_stats(path: &std::path::Path) -> u64 {
    // One stat for each parent through the first existing directory, then available_space.
    path.ancestors().position(|p| p.exists()).unwrap() as u64 + 2
}

pub(super) struct Model {
    expected: BTreeMap<(String, String), BTreeMap<Counter, u64>>,
    calls: BTreeMap<(String, String), u64>,
    volumes: BTreeSet<std::path::PathBuf>,
}
impl Model {
    pub(super) fn derive(store: &Store, assistant: &str) -> Self {
        let docs = documents(store, assistant);
        let bytes: Vec<_> = docs
            .iter()
            .map(|doc| serde_json::to_vec(doc).unwrap().len() as u64)
            .collect();
        let mut model = Self {
            expected: BTreeMap::new(),
            calls: BTreeMap::new(),
            volumes: BTreeSet::new(),
        };
        // PM inbox sweep 1 + root pass initial/reload 2 + outbox initial 1 +
        // ScopedPm readiness (update/load + native target/load) twice 4 + probe record 1
        // + delivery's unconditional bind/target load 1, even without pending messages.
        model.opaque("T-625", "root_pm_load", 10, bytes[0], 10);
        // Delivery: recover_publication's head, observe_pass's open head, replacement head.
        model.opaque("T-669", "standalone_inbox_load", 3, bytes[1], 3);
        // Assistant probe: initial container, mutation's session + container, always-save.
        // record() wraps the mutation in records_transaction: its session load opens the
        // held view; its following container load reuses it. Both documents still decode.
        model.opaque("T-671", "assistant_session_load", 1, bytes[2], 1);
        model.opaque("T-671", "assistant_container_load", 2, bytes[3], 1);
        model.opaque("T-671", "assistant_container_save", 1, bytes[3], 0);
        let tasks = store.list().unwrap();
        // The scalar location projection reads no canonical documents; only its one
        // view and the unchanged per-location filesystem walk contribute to this scope.
        let mut full: BTreeMap<Counter, u64> = Counter::ALL.into_iter().map(|c| (c, 0)).collect();
        full.insert(Counter::ViewsOpened, 1);
        let mut fixed = BTreeSet::from([store.dir().to_path_buf()]);
        for name in store.config.projects.keys() {
            let p = store.project_path(name).unwrap();
            fixed.extend([p.join(".worktrees"), p.join("target")]);
        }
        if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|p| !p.is_empty()) {
            let p = std::path::PathBuf::from(target);
            fixed.insert(if p.is_absolute() {
                p
            } else {
                std::env::current_dir().unwrap().join(p)
            });
        }
        let runs: Vec<_> = tasks.iter().flat_map(|t| t.runs.iter()).collect();
        let mut walk = BTreeSet::new();
        for run in &runs {
            if run.worktree.exists() {
                walk.extend([run.worktree.clone(), run.worktree.join("target")]);
            }
        }
        let stats = runs.len() as u64 + walk.difference(&fixed).map(|p| file_stats(p)).sum::<u64>();
        full.insert(Counter::FileStats, stats);
        // sync's persisted observation plus attention/snapshot_heads' resource precondition.
        // The latter resamples when no held-admission reason was passed, even on an empty board.
        for n in full.values_mut() {
            *n *= 2;
        }
        model.volumes = fixed.union(&walk).cloned().collect();
        model
            .expected
            .insert(("FU-3".into(), "sample".into()), full);
        model.calls.insert(("FU-3".into(), "sample".into()), 2);
        model
    }
    fn opaque(&mut self, id: &str, site: &str, calls: u64, bytes: u64, views: u64) {
        let expected = Counter::ALL
            .into_iter()
            .map(|c| {
                (
                    c,
                    match c {
                        Counter::ViewsOpened => views,
                        Counter::RecordDecodes => calls,
                        Counter::DecodedDocBytes => calls * bytes,
                        _ => 0,
                    },
                )
            })
            .collect();
        let key = (id.to_owned(), site.to_owned());
        self.expected.insert(key.clone(), expected);
        self.calls.insert(key, calls);
    }
    pub(super) fn volumes(&self, sample: &corral_core::ResourceSnapshot) {
        assert_eq!(
            sample
                .disks
                .iter()
                .map(|d| d.path.clone())
                .collect::<BTreeSet<_>>(),
            self.volumes
        );
    }
    pub(super) fn check(&self, delta: &WorkSnapshot) -> Vec<u64> {
        delta.check_identity().unwrap();
        assert_eq!(
            delta.followup_calls, self.calls,
            "exact producer calls; no extra scope"
        );
        assert_eq!(delta.unattributed_hydrations(), 0, "{delta:?}");
        for counter in Counter::ALL {
            let cell = delta.cells(counter);
            assert_eq!(cell.own_record, 0, "unexpected own-record {counter:?}");
            assert_eq!(cell.emitted, 0, "unexpected emitted {counter:?}");
            for (site, expected) in &self.expected {
                assert_eq!(
                    cell.followup.get(site).copied().unwrap_or(0),
                    expected[&counter],
                    "{site:?} {counter:?}"
                );
            }
            assert!(
                cell.followup
                    .keys()
                    .all(|site| self.expected.contains_key(site))
            );
        }
        Counter::ALL.into_iter().map(|c| delta.get(c)).collect()
    }
}
