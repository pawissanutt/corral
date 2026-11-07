//! Shared disposable co-work service fixture.
use super::{git, support};
use corral_core::assistant::AssistantScratch;
use corral_core::{AgentCmd, Status};
use corral_herdr::Herdr;
use corral_ops::{Ctx, assistant::scratch};
use serde_json::json;
use std::path::{Path, PathBuf};

pub(super) struct Fx {
    _tmp: tempfile::TempDir,
    pub(super) app: PathBuf,
    pub(super) ctx: Ctx,
}

impl Fx {
    /// Project `app` with a passing gate, reviewer `reviewer` when given, fake agent and
    /// reviewer profiles, and assistant session A-1.
    pub(super) fn new(reviewer: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reviewer_ops/herdr.sh"),
            &bin,
        )
        .unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "corral test"]);
        git(&app, &["config", "user.email", "corral@test.invalid"]);
        std::fs::write(app.join("README.md"), "hello\n").unwrap();
        std::fs::write(app.join(".gitignore"), ".worktrees/\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
        let mut store = corral_core::Store::init(tmp.path()).unwrap();
        for name in ["fake", "rev"] {
            let command = "true".into();
            store
                .config
                .agents
                .insert(name.into(), AgentCmd { command });
        }
        store.config.review.gate = Some(vec!["echo gate-ok".into()]);
        store.config.review.reviewer = reviewer.map(str::to_owned);
        let project = serde_json::from_value(json!({"path": "app"})).unwrap();
        store.config.projects.insert("app".into(), project);
        store.save_config().unwrap();
        store.create_assistant("pi", None, None, None).unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        let app = app.canonicalize().unwrap();
        Self {
            _tmp: tmp,
            app,
            ctx,
        }
    }

    pub(super) fn scratch(&self, slug: &str) -> AssistantScratch {
        scratch::create(&self.ctx, "A-1", "app", Some(slug), None).unwrap()
    }

    pub(super) fn commit(&self, dir: &Path, file: &str) -> String {
        std::fs::write(dir.join(file), format!("{file}\n")).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", &format!("add {file}")]);
        git(dir, &["rev-parse", "HEAD"])
    }

    pub(super) fn delegate_board(&mut self, id: &str) {
        let preview = self
            .ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [id.to_owned()].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        self.ctx.store.config = self
            .ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", corral_core::clock::now())
            .unwrap();
    }

    pub(super) fn kind(&self, slug: &str) -> Option<String> {
        let session = self.ctx.store.get_assistant("A-1").unwrap();
        let entry = session.scratch.into_iter().find(|s| s.slug == slug);
        entry.map(|s| s.kind)
    }

    /// Binds A-1's generation 1 to a Pi transcript with responses before, at and after `at`.
    pub(super) fn usage_around(&self, at: jiff::Timestamp) {
        let transcript = self.ctx.store.dir().join("A-1-session.jsonl");
        let line = |id: &str, ts: String, input: u64| {
            format!(
                "{{\"type\":\"message\",\"id\":\"{id}\",\"timestamp\":\"{ts}\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":{input},\"output\":1}}}}}}\n"
            )
        };
        let text = [
            "{\"type\":\"session\",\"id\":\"sid\"}\n".to_owned(),
            line("before", "2020-01-01T00:00:00Z".into(), 100),
            line("during", at.to_string(), 10),
            line("after", "2100-01-01T00:00:00Z".into(), 1000),
        ]
        .concat();
        std::fs::write(&transcript, text).unwrap();
        let store = &self.ctx.store;
        store
            .update_assistant_container("A-1", |container| {
                container.state.generations.push(serde_json::from_value(json!({
                    "n": 1, "bind_nonce_sha256": "nonce", "pane": "pane", "created": at,
                    "bound": {"host_cap_sha256": "host", "ctl_cap_sha256": "ctl", "session_id": "sid",
                        "session_file": transcript, "pid": 1, "start_time": "start",
                        "request_id": "hello", "at": at}
                }))?);
                container.state.binding.generation = 1;
                Ok(())
            })
            .unwrap();
    }
}
