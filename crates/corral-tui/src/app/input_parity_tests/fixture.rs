use super::*;

pub(super) struct Fixture {
    root: tempfile::TempDir,
    pub(super) ctx: Ctx,
    pub(super) id: String,
}

fn git(path: &Path, args: &[&str]) {
    let out = crate::board::tests::fixture_command("git", path)
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "corral parity test"]);
        git(&repo, &["config", "user.email", "parity@example.invalid"]);
        std::fs::write(repo.join("README.md"), "initial\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-qm", "init"]);
        let mut store = Store::init(root.path()).unwrap();
        store.config.herdr.session = Some("corral-test".into());
        store.config.defaults.profile = Some("fake".into());
        store.config.pm.agent = "fake".into();
        store.config.agents.clear();
        store.config.harnesses.clear();
        store.config.profiles.clear();
        store.config.harnesses.insert(
            "fake".into(),
            Harness {
                command: "sh -c 'printf \"parity change\\n\" > parity.txt' parity {model_args}"
                    .into(),
                flags: None,
                model_flag: Some("--model".into()),
                effort_flag: Some("--effort".into()),
                efforts: vec!["low".into(), "high".into()],
                prompt: None,
            },
        );
        for (name, model, effort) in [("fake", "model-one", "low"), ("spare", "model-two", "high")]
        {
            store.config.profiles.insert(
                name.into(),
                Profile {
                    harness: "fake".into(),
                    model: Some(model.into()),
                    vendor: None,
                    escalate_to: None,
                    effort: Some(effort.into()),
                    args: Vec::new(),
                    metadata: None,
                },
            );
        }
        store.config.review.accept = Some(Accept::Auto);
        store.config.review.gate = Some(vec!["true".into()]);
        store.save_config().unwrap();
        // Start with the persisted stamp so returning from detail cannot reload
        // config and replace the explicitly injected transport.
        let mut store = Store::open(root.path()).unwrap();
        let id = store
            .create("Input parity", Some("app"), Status::Ready, "Parity spec\n")
            .unwrap()
            .id;
        store
            .update(&id, |task| {
                task.review = Some(Accept::Owner);
                Ok(())
            })
            .unwrap();
        let preview = store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [id.clone()].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store.config = store
            .confirm_operating_policy(
                &preview,
                "fixture owner",
                "2026-10-15T12:00:00Z".parse().unwrap(),
            )
            .unwrap();
        let report = HarnessReport {
            detected_at: "2026-10-15T12:00:00Z".parse().unwrap(),
            harnesses: vec![HarnessInfo {
                key: "fake".into(),
                program: Some("sh".into()),
                path: Some("/bin/sh".into()),
                version: Some("fixture".into()),
                models: vec!["model-one".into(), "model-two".into()],
                model_source: ModelSource::Listed,
                efforts: vec!["low".into(), "high".into()],
                takes_effort: true,
                notes: Vec::new(),
            }],
        };
        std::fs::write(
            store.state_dir().join(corral_ops::HARNESS_CACHE_FILE),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        let bin = root.path().join("herdr");
        std::fs::write(&bin, include_str!("../../../testdata/parity-herdr.sh")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Self { root, ctx, id }
    }

    pub(super) fn app(&self) -> App {
        let now = "2026-10-15T12:00:00Z".parse().unwrap();
        let mut app = App::with_board(self.ctx.clone(), Mode::Board, |ctx| {
            BoardView::with_clock(ctx, Box::new(move || now))
        })
        .unwrap();
        app.clock = Box::new(FakeClock(Rc::new(Cell::new(Instant::now()))));
        app
    }

    pub(super) fn status(&self) -> Status {
        self.ctx.store.get(&self.id).unwrap().status
    }

    pub(super) fn agent_finishes(&self, app: &mut App, profile: &str, model: &str, effort: &str) {
        let task = self.ctx.store.get(&self.id).unwrap();
        assert_eq!(task.status, Status::Running);
        let run = task.current_run().unwrap();
        assert_eq!(
            (
                run.agent.as_str(),
                run.model.as_deref(),
                run.effort.as_deref(),
                run.mode
            ),
            (profile, Some(model), Some(effort), RunMode::Interactive)
        );
        // The fake executes the command sent through pane_run in the real dispatched worktree.
        assert_eq!(
            std::fs::read_to_string(run.worktree.join("parity.txt")).unwrap(),
            "parity change\n"
        );
        git(&run.worktree, &["add", "parity.txt"]);
        git(&run.worktree, &["commit", "-qm", "Record fixture progress"]);
        let state = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            &self.id,
            &CheckpointUpdate {
                task: self.id.clone(),
                run: state.run,
                spec_revision: state.spec_revision,
                expected_revision: state
                    .checkpoints
                    .last()
                    .map_or(0, |c| c.checkpoint.revision),
                progress_revision: 0,
                head: state.head,
                base: state.base,
                tree: state.tree,
                criterion_ids: Vec::new(),
                evidence_refs: Vec::new(),
                next_action: "Complete the input parity lifecycle".into(),
                decisions: Vec::new(),
                waits: Vec::new(),
                resources: Vec::new(),
            },
        )
        .unwrap();
        corral_ops::done(&self.ctx, &self.id, "Parity run complete").unwrap();
        app.on_worker(WorkerMsg::synced(
            &app.ctx,
            Ok(corral_ops::sync(&self.ctx).unwrap()),
        ));
        assert_eq!(self.status(), Status::NeedsMe);
        assert_eq!(
            self.ctx
                .store
                .get(&self.id)
                .unwrap()
                .current_run()
                .unwrap()
                .autopilot
                .as_ref()
                .unwrap()
                .stage,
            AutoStage::Escalated
        );
        assert!(render(app).contains("autopilot:"));
    }

    pub(super) fn assert_landed(&self, app: &mut App) {
        assert_eq!(self.status(), Status::Done);
        let text = render(app);
        let card = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Card(self.id.clone()))
            .unwrap()
            .0;
        let done = app
            .hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Column(Status::Done))
            .unwrap()
            .0;
        assert!(done.contains((card.x, card.y).into()), "{text}");
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("app/parity.txt")).unwrap(),
            "parity change\n"
        );
        let task = self.ctx.store.get(&self.id).unwrap();
        assert_eq!(task.runs.len(), 2);
        assert!(!task.runs[1].worktree.exists());
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("closed")).unwrap(),
            "w1:p1\n"
        );
        let events = self.ctx.store.events().unwrap();
        for (kind, count) in [("dispatched", 2), ("done", 2), ("approved", 1)] {
            assert_eq!(events.iter().filter(|e| e.kind == kind).count(), count);
        }
    }
}
