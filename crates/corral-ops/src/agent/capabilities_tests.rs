//! Capability framework (T-563): one resolved snapshot per generation, adaptive explicit
//! resume, legacy records and an older installed host, through the real dispatch path.
use std::path::Path;

use corral_core::agent::{AgentRole, CapabilityChange, SnapshotPreset};
use corral_core::inbox::Recipient;
use corral_core::{Status, Store};
use serde_json::{Value, json};

use super::BindTarget;
use super::cli::AgentTarget;
use super::launch::{
    Bootstrap, HOST_ENTRY, HostSpec, Secrets, check_with, consume_bootstrap, generation_dir,
    launch_message, prepare_bootstrap, runtime_dir,
};
use crate::Ctx;

struct Fixture {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    task: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(dir) = runtime_dir(&self.ctx.store, &self.recipient()) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A store whose fixture installation's manifest lists `capabilities` (absent: an older host).
fn fixture(capabilities: Option<&[&str]>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let assets = tmp.path().join("assets");
    let mut compat = json!({"version": 1, "verdict": "GO", "bun": {"range": "1.4.2"}});
    if let Some(keys) = capabilities {
        compat["capabilities"] = json!(keys);
    }
    write(&assets.join("compat.json"), &compat.to_string());
    for file in ["package.json", "bun.lock"] {
        write(&assets.join(file), "{}\n");
    }
    for module in [
        HOST_ENTRY,
        "src/roles/worker/index.ts",
        "src/delivery/index.ts",
        "src/auth/index.ts",
    ] {
        write(&assets.join(module), "export default {};\n");
    }
    write(&assets.join("fixture.json"), "{\"fixture\": true}\n");
    let mut config = std::fs::read_to_string(store.config_path()).unwrap();
    config.push_str(&format!(
        "\n[agent]\nassets_dir = {:?}\n",
        assets.display().to_string()
    ));
    std::fs::write(store.config_path(), config).unwrap();
    let ctx = Ctx::from_store(Store::open(tmp.path()).unwrap());
    let task = ctx
        .store
        .create("Pi worker", None, Status::Running, "")
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.project = Some("corral".into());
            t.runs.push(super::test_support::run("R1", "p1"));
            Ok(())
        })
        .unwrap();
    Fixture {
        tmp,
        ctx,
        task: task.id,
    }
}

impl Fixture {
    fn recipient(&self) -> Recipient {
        Recipient::Run {
            task: self.task.clone(),
            run: "R1".into(),
        }
    }

    fn target(&self) -> BindTarget {
        BindTarget::Worker {
            task: self.task.clone(),
            run: "R1".into(),
        }
    }

    fn spec(&self) -> HostSpec {
        HostSpec {
            cwd: self.tmp.path().join("worktree"),
            model: "gpt-6.1-sol".into(),
            provider: None,
            effort: None,
            prompt_file: None,
            adopt: false,
        }
    }

    /// Opens the next generation as dispatch or an explicit resume does.
    fn open(&self) -> anyhow::Result<Bootstrap> {
        let prepared = prepare_bootstrap(&self.ctx, &self.target(), "p1", &self.spec())?;
        consume_bootstrap(&prepared.bootstrap)
    }

    fn host(&self) -> Box<corral_core::agent::AgentRunState> {
        let task = self.ctx.store.get(&self.task).unwrap();
        task.current_run().unwrap().agent_host.clone().unwrap()
    }

    fn state(&self) -> Value {
        let target = AgentTarget::Task {
            task: self.task.clone(),
            run: None,
            review: None,
        };
        super::install::state_value(&self.ctx, &target, &json!({})).unwrap()["container"].clone()
    }

    fn message(&self, bootstrap: &Bootstrap) -> Value {
        let bun = self.tmp.path().join("bun");
        super::test_support::write_executable(&bun, "#!/bin/sh\necho 1.4.2\n");
        let dir = generation_dir(&self.ctx.store, &self.recipient(), bootstrap.generation).unwrap();
        let ready = check_with(bootstrap, &dir, None, Some(bun)).unwrap();
        serde_json::from_slice(
            &launch_message(bootstrap, &ready, &Secrets::generate().unwrap()).unwrap(),
        )
        .unwrap()
    }

    fn capability_check(&self) -> super::hooks::doctor::DoctorCheck {
        super::hooks::doctor::checks_with(&self.ctx, None, None)
            .unwrap()
            .into_iter()
            .find(|check| check.id == "pi_native_capabilities")
            .unwrap()
    }
}

#[test]
fn explicit_resume_adapts_changed_settings_and_keeps_each_generation_snapshot() {
    // Given a current host and a first generation under the absent-config default.
    let mut fx = fixture(Some(&["todo"]));
    let first = fx.open().unwrap();
    let snapshot = first.capabilities.clone().unwrap();
    assert!(first.todo && snapshot.enabled("todo"));
    assert_eq!(snapshot.preset, SnapshotPreset::Standard);
    let message = fx.message(&first);
    assert_eq!(message["todo"], true);
    assert_eq!(
        message["capabilities"],
        json!({"catalog": corral_core::agent::CAPABILITY_CATALOG_VERSION, "digest": snapshot.digest,
            "preset": "standard", "enabled": ["todo"], "settings": {}, "project_text": true,
            "project_dir": null})
    );
    // When the owner disables worker todo and the run is explicitly resumed.
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set(AgentRole::Worker, "todo", false)
        .unwrap();
    let second = fx.open().unwrap();
    // Then the new generation adapts, records the concrete difference, and g1 is unchanged.
    let host = fx.host();
    let (g1, g2) = (host.generation(1).unwrap(), host.generation(2).unwrap());
    assert_eq!(g1.capabilities.as_ref(), Some(&snapshot));
    assert!(g1.capability("todo") && !g2.capability("todo"));
    assert!(!second.todo);
    assert_eq!(fx.message(&second)["capabilities"]["enabled"], json!([]));
    let transition = g2.capability_transition.clone().unwrap();
    assert_eq!(
        (transition.from_generation, transition.from_digest.as_str()),
        (1, snapshot.digest.as_str())
    );
    assert_eq!(
        transition.changes,
        vec![CapabilityChange {
            key: "todo".into(),
            from: Some(true),
            to: Some(false),
            settings: false,
        }]
    );
    // And state reads the same snapshot and change, while an old bootstrap shape still loads.
    let state = fx.state();
    assert_eq!(
        state["capabilities"]["digest"],
        json!(second.capabilities.as_ref().unwrap().digest)
    );
    assert_eq!(state["capability_change"]["changes"][0]["key"], "todo");
    let mut old = serde_json::to_value(&second).unwrap();
    old.as_object_mut().unwrap().remove("capabilities");
    assert_eq!(
        serde_json::from_value::<Bootstrap>(old)
            .unwrap()
            .capabilities,
        None
    );
}

#[test]
fn a_legacy_generation_is_labeled_and_resumes_without_reconstruction() {
    // Given a generation recorded before the catalog (no snapshot, todo off).
    let fx = fixture(Some(&["todo"]));
    fx.open().unwrap();
    fx.ctx
        .store
        .update(&fx.task, |task| {
            let host = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
            let generation = host.generation_mut(1).unwrap();
            generation.capabilities = None;
            generation.todo = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(fx.state()["capabilities"]["preset"], "legacy_strict");
    // When the run is explicitly resumed under current settings.
    let resumed = fx.open().unwrap();
    // Then it adapts instead of refusing, and the transition names the legacy origin.
    assert!(resumed.todo);
    let host = fx.host();
    let transition = host
        .generation(2)
        .unwrap()
        .capability_transition
        .clone()
        .unwrap();
    assert_eq!(transition.from_preset, SnapshotPreset::LegacyStrict);
    // This fixture host implements only todo, so the other keys' changes are not the point here.
    assert_eq!(
        transition
            .changes
            .iter()
            .find(|change| change.key == "todo"),
        Some(&CapabilityChange {
            key: "todo".into(),
            from: Some(false),
            to: Some(true),
            settings: false,
        })
    );
    assert!(host.generation(1).unwrap().capabilities.is_none());
}

#[test]
fn an_older_installed_host_omits_unavailable_defaults_and_refuses_explicit_requests() {
    // Given an installed host whose manifest predates the catalog.
    let mut fx = fixture(None);
    // When a run opens under the default preset.
    let bootstrap = fx.open().unwrap();
    // Then todo is reported unavailable and omitted rather than refusing the launch.
    let snapshot = bootstrap.capabilities.clone().unwrap();
    assert!(!bootstrap.todo && !snapshot.enabled("todo"));
    let reason = snapshot.features["todo"].unavailable.clone().unwrap();
    assert!(reason.contains("corral agent install"));
    let check = fx.capability_check();
    assert!(check.ok);
    assert!(check.message.contains("todo unavailable"));
    // When the owner explicitly requests todo for the worker.
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set(AgentRole::Worker, "todo", true)
        .unwrap();
    let before = fx.host().generations.len();
    let refused = fx.open().unwrap_err();
    // Then the launch is refused with a targeted fix and no generation is opened.
    let text = format!("{refused:#}");
    assert!(text.contains("capability_unavailable: [agent.capabilities.worker] todo = true"));
    assert!(text.contains("corral agent install"));
    assert_eq!(fx.host().generations.len(), before);
    let check = fx.capability_check();
    assert!(!check.ok);
    assert!(
        check
            .fix
            .unwrap()
            .contains("[agent.capabilities.worker] todo")
    );
}

#[test]
fn web_settings_resume_preserves_prior_snapshot_and_delivers_resolved_policy() {
    // Given an installed web host and a standard generation with default policy.
    let mut fx = fixture(Some(&["todo", "web.enabled"]));
    let first = fx.open().unwrap();
    let snapshot = first.capabilities.clone().unwrap();
    assert_eq!(
        fx.message(&first)["capabilities"]["web"],
        json!({"hosts": ["*"], "max_bytes": 1048576, "timeout_ms": 15000, "max_redirects": 5})
    );
    let mut config = std::fs::read_to_string(fx.ctx.store.config_path()).unwrap();
    config.push_str(
        "\n[agent.capabilities.worker.web]\nhosts = ['docs.example.org']\n\
         max_bytes = 4096\ntimeout_ms = 2000\nmax_redirects = 0\n\
         searxng_url = 'http://localhost:8080/search'\n",
    );
    std::fs::write(fx.ctx.store.config_path(), config).unwrap();
    fx.ctx = Ctx::from_store(Store::open(fx.tmp.path()).unwrap());
    // When an explicit resume resolves the changed policy.
    let second = fx.open().unwrap();
    // Then only the new generation and launch policy change, with a visible settings diff.
    let resumed = second.capabilities.as_ref().unwrap();
    assert_ne!(snapshot.digest, resumed.digest);
    assert_eq!(
        fx.message(&second)["capabilities"]["web"],
        json!({"hosts": ["docs.example.org"], "max_bytes": 4096, "timeout_ms": 2000,
            "max_redirects": 0, "searxng_url": "http://localhost:8080/search"})
    );
    let host = fx.host();
    assert_eq!(
        host.generation(1).unwrap().capabilities.as_ref(),
        Some(&snapshot)
    );
    let change = host
        .generation(2)
        .unwrap()
        .capability_transition
        .as_ref()
        .unwrap();
    assert!(change.changes.is_empty());
    let web = change.web_change.as_ref().unwrap();
    assert_eq!(web.from, snapshot.web);
    assert_eq!(web.to, resumed.web);
    assert_eq!(
        fx.state()["capability_change"]["web_change"]["to"]["max_bytes"],
        4096
    );
}

#[test]
fn hook_and_bash_rule_settings_reach_launch_and_resume_records_their_change() {
    // Given a host implementing hooks and bash rules, and a standard generation without them.
    let mut fx = fixture(Some(&["todo", "hooks", "bash_rules"]));
    let first = fx.open().unwrap();
    let settings = &fx.message(&first)["capabilities"]["settings"];
    assert!(settings.get("hooks").is_none() && settings.get("bash_rules").is_none());
    // When the owner enables both for the worker and explicitly resumes.
    let mut config = std::fs::read_to_string(fx.ctx.store.config_path()).unwrap();
    config.push_str(
        "\n[agent.capabilities.worker.hooks]\nenabled = true\ntimeout_ms = 900\n\
         [[agent.capabilities.worker.hooks.post_tool]]\nname = 'fmt'\nargv = ['cargo', 'fmt']\n\
         tools = ['edit']\n[agent.capabilities.worker.bash_rules]\nenabled = true\n\
         deny = ['git push*']\n",
    );
    std::fs::write(fx.ctx.store.config_path(), config).unwrap();
    fx.ctx = Ctx::from_store(Store::open(fx.tmp.path()).unwrap());
    let second = fx.open().unwrap();
    // Then the launch carries the resolved settings and the transition names both keys.
    let capabilities = &fx.message(&second)["capabilities"];
    assert_eq!(
        capabilities["settings"]["hooks"],
        json!({"timeout_ms": 900, "pre_tool": [], "turn_end": [],
            "post_tool": [{"name": "fmt", "argv": ["cargo", "fmt"], "tools": ["edit"]}]})
    );
    assert_eq!(
        capabilities["settings"]["bash_rules"],
        json!({"allow": [], "ask": [], "deny": ["git push*"], "default": "allow"})
    );
    assert!(
        capabilities["enabled"]
            .as_array()
            .unwrap()
            .contains(&json!("hooks"))
    );
    let host = fx.host();
    assert_eq!(host.generation(1).unwrap().capabilities, first.capabilities);
    let change = host
        .generation(2)
        .unwrap()
        .capability_transition
        .clone()
        .unwrap();
    let keys: Vec<&str> = change.changes.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, ["bash_rules", "hooks"]);
}

#[test]
fn disabled_unsupported_and_historical_web_settings_are_omitted_from_launch() {
    // Given a host without web support and a historical snapshot without settings.
    let mut fx = fixture(Some(&["todo"]));
    let mut historical = fx.open().unwrap();
    let mut value = serde_json::to_value(historical.capabilities.as_ref().unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("web");
    value["catalog"] = json!(1);
    value["digest"] = json!("historical-digest");
    value["features"]
        .as_object_mut()
        .unwrap()
        .remove("web.enabled");
    historical.capabilities = Some(serde_json::from_value(value).unwrap());
    // When launch payloads are generated for unsupported, historical, and disabled settings.
    assert!(fx.message(&historical)["capabilities"].get("web").is_none());
    let unsupported = fx.open().unwrap();
    assert!(unsupported.capabilities.as_ref().unwrap().web.is_none());
    assert!(
        fx.message(&unsupported)["capabilities"]
            .get("web")
            .is_none()
    );
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set(AgentRole::Worker, "web.enabled", false)
        .unwrap();
    let disabled = fx.open().unwrap();
    // Then no policy is delivered and the historical digest remains usable unchanged.
    assert!(fx.message(&disabled)["capabilities"].get("web").is_none());
    assert_eq!(
        historical.capabilities.as_ref().unwrap().digest,
        "historical-digest"
    );
}

#[test]
fn the_pm_bootstrap_names_its_selected_project_checkout_for_guidance() {
    // Given a registered project and a PM launched at the workspace root for it.
    let mut fx = fixture(Some(&["todo"]));
    let app = fx.tmp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path": "app"})).unwrap(),
    );
    write(
        &fx.tmp.path().join("assets/src/roles/pm/index.ts"),
        "export default {};\n",
    );
    let pm = BindTarget::Pm {
        project: "corral".into(),
    };
    let spec = HostSpec {
        cwd: fx.tmp.path().to_path_buf(),
        ..fx.spec()
    };
    // When the PM and a worker bootstrap are prepared.
    let prepared = prepare_bootstrap(&fx.ctx, &pm, "p1", &spec).unwrap();
    let bootstrap = consume_bootstrap(&prepared.bootstrap).unwrap();
    let worker = fx.open().unwrap();
    // Then only the PM names the project checkout, separately from its workspace cwd.
    assert_eq!(bootstrap.cwd, fx.tmp.path());
    assert_eq!(bootstrap.project_dir.as_deref(), Some(app.as_path()));
    assert_eq!(bootstrap.capabilities.unwrap().project_text, Some(true));
    // The worker's launch message pins "project_dir": null in the exact-message test above.
    assert_eq!(worker.project_dir, None);
}

#[test]
fn background_shell_bounds_reach_the_worker_launch_only_when_installed() {
    // Given an installed host with background_shell and one without it.
    let fx = fixture(Some(&["todo", "background_shell"]));
    let current = fx.open().unwrap();
    // Then the resolved bounds travel with the snapshot settings only where the host has the key.
    assert_eq!(
        fx.message(&current)["capabilities"]["settings"]["background_shell"],
        json!({"max_jobs": 4, "max_output_bytes": 8388608, "max_runtime_secs": 3600})
    );
    assert!(
        fx.message(&current)["capabilities"]["enabled"]
            .as_array()
            .unwrap()
            .contains(&json!("background_shell"))
    );
    let older = fixture(Some(&["todo"]));
    let omitted = older.open().unwrap();
    let message = older.message(&omitted);
    assert!(
        message["capabilities"]["settings"]
            .get("background_shell")
            .is_none()
    );
    assert!(
        omitted.capabilities.as_ref().unwrap().features["background_shell"]
            .unavailable
            .is_some()
    );
}
