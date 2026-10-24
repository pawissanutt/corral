//! The launcher (`agent/launch.rs`): the dispatch-side bootstrap, its one-shot consumption, the
//! ordered pre-exec refusals and the inherited launch pipe. Kept beside the bridge so the
//! launcher's tests stay in host-bridge's own scope.
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use corral_core::agent::paths;
use corral_core::inbox::Recipient;
use corral_core::{Status, Store};
use serde_json::{Value, json};

use super::*;
use crate::Ctx;
use crate::agent::test_support::run;

const ALL_MODULES: [&str; 3] = [
    "src/roles/worker/index.ts",
    "src/delivery/index.ts",
    "src/auth/index.ts",
];

struct Fixture {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    assets: PathBuf,
    task: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(dir) = runtime_dir(&self.ctx.store, &self.recipient()) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Fixture {
    fn recipient(&self) -> Recipient {
        Recipient::Run {
            task: self.task.clone(),
            run: "R1".into(),
        }
    }

    fn generation_dir(&self) -> PathBuf {
        generation_dir(&self.ctx.store, &self.recipient(), 1).unwrap()
    }

    fn spec(&self) -> HostSpec {
        HostSpec {
            cwd: self._tmp.path().join("worktree"),
            model: "gpt-6.1-sol".into(),
            effort: Some("high".into()),
            prompt_file: None,
        }
    }

    fn dispatch(&self) -> (String, PathBuf, Bootstrap) {
        let target = BindTarget::Worker {
            task: self.task.clone(),
            run: "R1".into(),
        };
        let command = prepare(&self.ctx, &target, "p1", &self.spec()).unwrap();
        let path = self.generation_dir().join(BOOTSTRAP_FILE);
        let bootstrap = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        (command, path, bootstrap)
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn asset_dir(root: &Path, name: &str, modules: &[&str], fixture: bool) -> PathBuf {
    let dir = root.join(name);
    write(&dir.join("package.json"), "{\"name\":\"corral-pi\"}\n");
    write(&dir.join("bun.lock"), "{}\n");
    let compat = json!({"version": 1, "verdict": "GO", "bun": {"range": "1.4.2"}});
    write(&dir.join("compat.json"), &compat.to_string());
    write(&dir.join(HOST_ENTRY), "// host\n");
    for module in modules {
        write(&dir.join(module), "export default {};\n");
    }
    if fixture {
        write(&dir.join("fixture.json"), "{\"fixture\": true}\n");
    }
    dir
}

/// A store whose `[agent] assets_dir` names a fixture installation, with a running task R1.
fn fixture(modules: &[&str], fixture: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let assets = asset_dir(tmp.path(), "assets", modules, fixture);
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
            t.runs.push(run("R1", "p1"));
            Ok(())
        })
        .unwrap();
    Fixture {
        _tmp: tmp,
        ctx,
        assets,
        task: task.id,
    }
}

fn fake_bun(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join(format!("bun-{version}"));
    write(
        &path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo {version}; exit 0; fi\n\
             out=\"$CORRAL_FAKE_BUN_OUT\"\nprintf '%s\\n' \"$@\" > \"$out/argv\"\nenv > \"$out/env\"\n\
             pwd > \"$out/cwd\"\ncat \"/proc/self/fd/$4\" > \"$out/message\"\n"
        ),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn refusal(result: Result<Ready, LaunchRefusal>) -> LaunchRefusal {
    result.expect_err("the launch must be refused")
}

#[test]
fn dispatch_writes_a_one_shot_0600_bootstrap_in_the_0700_runtime_directory() {
    let fx = fixture(&ALL_MODULES, true);
    let (command, path, bootstrap) = fx.dispatch();
    let corral = crate::prompt::corral_bin().unwrap();
    assert_eq!(
        command,
        format!(
            "'{}' agent launch --bootstrap '{}' --assets '{}'",
            corral.display(),
            path.display(),
            fx.assets.display()
        )
    );
    let file = std::fs::symlink_metadata(&path).unwrap();
    assert_eq!(file.mode() & 0o777, 0o600);
    assert_eq!(file.uid(), rustix::process::getuid().as_raw());
    for dir in [
        fx.generation_dir(),
        runtime_dir(&fx.ctx.store, &fx.recipient()).unwrap(),
    ] {
        assert_eq!(std::fs::metadata(dir).unwrap().mode() & 0o777, 0o700);
    }
    assert_eq!(bootstrap.role, AgentRole::Worker);
    assert_eq!(
        (bootstrap.task.as_deref(), bootstrap.run.as_deref()),
        (Some(fx.task.as_str()), Some("R1"))
    );
    assert_eq!(bootstrap.generation, 1);
    assert_eq!(bootstrap.store, paths::store_id(&fx.ctx.store));
    assert_eq!(bootstrap.asset_sha256, asset_digest(&fx.assets).unwrap());
    let container = ContainerRef::Worker {
        task: fx.task.clone(),
        run: "R1".into(),
    };
    assert_eq!(
        bootstrap.session_dir,
        paths::container_sessions_dir(&fx.ctx.store, &container)
    );
    let task = fx.ctx.store.get(&fx.task).unwrap();
    let generation = corral_core::agent::container_state(&task, &container).unwrap();
    assert_eq!(
        generation.generation(1).unwrap().bind_nonce_sha256,
        paths::sha256_hex(bootstrap.bind_nonce.as_bytes())
    );
    assert!(!command.contains(&bootstrap.bind_nonce));
    assert!(format!("{bootstrap:?}").contains("<redacted>"));
    assert!(!format!("{bootstrap:?}").contains(&bootstrap.bind_nonce));
}

#[test]
fn the_worker_launch_seam_prepares_the_current_run_with_its_model_and_prompt() {
    let fx = fixture(&ALL_MODULES, true);
    fx.ctx
        .store
        .update(&fx.task, |t| {
            t.runs[0].model = Some("gpt-6.1-sol".into());
            Ok(())
        })
        .unwrap();
    let task = fx.ctx.store.get(&fx.task).unwrap();
    let command =
        crate::agent::hooks::launch::worker_command(&fx.ctx, &task, &task.runs[0]).unwrap();
    assert!(command.contains(" agent launch --bootstrap "));
    let bootstrap: Bootstrap =
        serde_json::from_slice(&std::fs::read(fx.generation_dir().join(BOOTSTRAP_FILE)).unwrap())
            .unwrap();
    assert_eq!(bootstrap.model, "gpt-6.1-sol");
    assert_eq!(
        bootstrap.prompt_file,
        Some(
            fx.ctx
                .store
                .prompts_dir()
                .join(format!("{}-R1.md", fx.task))
        )
    );
    assert_eq!(bootstrap.cwd, task.runs[0].worktree);
}

#[test]
fn launch_consumes_the_bootstrap_and_refuses_a_fixture_not_named_by_assets_dir() {
    let fx = fixture(&ALL_MODULES, true);
    let (_, path, _) = fx.dispatch();
    let other = asset_dir(fx._tmp.path(), "elsewhere", &ALL_MODULES, true);
    let args = crate::agent::cli::LaunchArgs {
        bootstrap: path.clone(),
        assets: Some(other),
    };
    let error = launch(fx._tmp.path(), &args).unwrap_err();
    let refused = error.downcast_ref::<LaunchRefusal>().unwrap();
    assert_eq!(refused.code, "fixture_assets_not_allowed");
    assert!(!path.exists(), "the bootstrap is one-shot");
    assert!(
        !fx.generation_dir().join(CTL_CAP_FILE).exists(),
        "no secret is generated"
    );
    let again = launch(fx._tmp.path(), &args).unwrap_err();
    assert_eq!(
        again.downcast_ref::<LaunchRefusal>().unwrap().code,
        "bootstrap_unavailable"
    );
}

#[test]
fn assets_without_modules_refuse_naming_every_missing_module_and_its_owner() {
    let fx = fixture(&[], false);
    let (_, path, bootstrap) = fx.dispatch();
    let bun = fake_bun(fx._tmp.path(), "1.4.2");
    let refused = refusal(check_with(
        &bootstrap,
        &fx.generation_dir(),
        None,
        Some(bun.clone()),
    ));
    assert_eq!(refused.code, "role_unavailable");
    assert_eq!(
        refused.detail,
        "role_unavailable worker (owner worker-role); delivery_unavailable (owner inbox-adapter); \
         credentials_unavailable (owner host-install)"
    );
    let only_auth = fixture(&ALL_MODULES[..2], true);
    let (_, _, bootstrap) = only_auth.dispatch();
    let refused = refusal(check_with(
        &bootstrap,
        &only_auth.generation_dir(),
        None,
        Some(bun),
    ));
    assert_eq!(
        (refused.code, refused.detail.as_str()),
        (
            "credentials_unavailable",
            "credentials_unavailable (owner host-install)"
        )
    );
    assert!(path.exists(), "check alone never consumes the bootstrap");
}

#[test]
fn no_go_manifest_an_unsupported_bun_or_changed_assets_refuse_with_the_exact_reason() {
    let fx = fixture(&ALL_MODULES, true);
    let (_, _, bootstrap) = fx.dispatch();
    let good = fake_bun(fx._tmp.path(), "1.4.2");
    assert!(check_with(&bootstrap, &fx.generation_dir(), None, Some(good.clone())).is_ok());
    let old = fake_bun(fx._tmp.path(), "1.4.1");
    let refused = refusal(check_with(
        &bootstrap,
        &fx.generation_dir(),
        None,
        Some(old.clone()),
    ));
    assert_eq!(refused.code, "bun_version_unsupported");
    assert_eq!(
        refused.detail,
        format!(
            "bun 1.4.1 at {} is outside the GO manifest's tested range 1.4.2",
            old.display()
        )
    );
    assert_eq!(
        refusal(check_with(&bootstrap, &fx.generation_dir(), None, None)).code,
        "bun_unavailable"
    );
    write(
        &fx.assets.join("src/roles/worker/index.ts"),
        "export default { changed: true };\n",
    );
    let refused = refusal(check_with(
        &bootstrap,
        &fx.generation_dir(),
        None,
        Some(good.clone()),
    ));
    assert_eq!(refused.code, "asset_digest_changed");
    assert!(refused.detail.contains(&bootstrap.asset_sha256));
    write(
        &fx.assets.join("compat.json"),
        "{\"verdict\": \"NO-GO\", \"bun\": {\"range\": \"1.4.2\"}}",
    );
    assert_eq!(
        refusal(check_with(
            &bootstrap,
            &fx.generation_dir(),
            None,
            Some(good.clone())
        ))
        .code,
        "compat_unavailable"
    );
    std::fs::remove_file(fx.assets.join("compat.json")).unwrap();
    let refused = refusal(check_with(
        &bootstrap,
        &fx.generation_dir(),
        None,
        Some(good),
    ));
    assert_eq!(refused.code, "compat_unavailable");
    assert!(
        refused
            .to_string()
            .starts_with("corral agent launch refused: compat_unavailable: ")
    );
}

#[test]
fn the_host_inherits_the_launch_message_on_a_pipe_and_no_secret_reaches_argv_or_env() {
    let fx = fixture(&ALL_MODULES, true);
    let (_, path, _) = fx.dispatch();
    let bootstrap = consume_bootstrap(&path).unwrap();
    assert!(!path.exists());
    let ready = check_with(
        &bootstrap,
        &fx.generation_dir(),
        None,
        Some(fake_bun(fx._tmp.path(), "1.4.2")),
    )
    .unwrap();
    let secrets = Secrets::generate().unwrap();
    assert_ne!(secrets.host_cap, secrets.ctl_cap);
    assert!(secrets.ctl_cap.len() == 64 && secrets.ctl_cap.bytes().all(|b| b.is_ascii_hexdigit()));
    write_ctl_cap(&ready.generation_dir, &secrets.ctl_cap).unwrap();
    let cap = ready.generation_dir.join(CTL_CAP_FILE);
    assert_eq!(std::fs::metadata(&cap).unwrap().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read_to_string(&cap).unwrap(), secrets.ctl_cap);
    let out = fx._tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let (mut command, reader) = host_command(&bootstrap, &ready, &secrets).unwrap();
    let status = command.env("CORRAL_FAKE_BUN_OUT", &out).status().unwrap();
    drop(reader);
    assert!(status.success());
    let read = |name: &str| std::fs::read_to_string(out.join(name)).unwrap();
    let argv: Vec<String> = read("argv").lines().map(str::to_owned).collect();
    assert_eq!(
        argv[..3],
        [
            "--no-env-file".to_owned(),
            fx.assets.join(HOST_ENTRY).display().to_string(),
            "--launch-fd".to_owned()
        ]
    );
    assert!(argv[3].parse::<u32>().unwrap() > 2);
    assert_eq!(
        read("cwd").trim(),
        fx.assets.canonicalize().unwrap().display().to_string()
    );
    let message: Value = serde_json::from_str(&read("message")).unwrap();
    assert_eq!(message["secrets"]["host_cap"], json!(secrets.host_cap));
    assert_eq!(message["secrets"]["ctl_cap"], json!(secrets.ctl_cap));
    assert_eq!(
        message["secrets"]["bind_nonce"],
        json!(bootstrap.bind_nonce)
    );
    assert_eq!(message["generation"], json!(1));
    assert_eq!(message["runtime_dir"], json!(ready.generation_dir));
    for secret in [&secrets.host_cap, &secrets.ctl_cap, &bootstrap.bind_nonce] {
        assert!(!read("argv").contains(secret.as_str()));
        assert!(!read("env").contains(secret.as_str()));
    }
}

#[test]
fn a_moved_loose_or_missing_bootstrap_is_refused() {
    let fx = fixture(&ALL_MODULES, true);
    let (_, path, _) = fx.dispatch();
    let moved = fx._tmp.path().join("moved.json");
    std::fs::copy(&path, &moved).unwrap();
    std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o600)).unwrap();
    let code = |p: &Path| {
        consume_bootstrap(p)
            .unwrap_err()
            .downcast::<LaunchRefusal>()
            .unwrap()
            .code
    };
    assert_eq!(code(&moved), "bootstrap_unsafe");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(code(&path), "bootstrap_unsafe");
    assert_eq!(
        code(&fx.generation_dir().join("absent.json")),
        "bootstrap_unavailable"
    );
}

#[test]
fn version_ranges_accept_exact_comparator_and_alternative_forms() {
    use super::super::bridge::assets::version_satisfies as ok;
    assert!(ok("1.4.2", "1.4.2") && !ok("1.4.1", "1.4.2") && !ok("1.4.3", "=1.4.2"));
    assert!(ok("1.4.5", ">=1.4.2 <1.5.0") && !ok("1.5.0", ">=1.4.2 <1.5.0"));
    assert!(ok("1.3.0", "1.2.0 || 1.3.0") && !ok("1.4.2-canary.1", "1.4.3"));
    assert!(!ok("not-a-version", "1.4.2") && !ok("1.4.2", ""));
}

#[test]
fn the_asset_digest_covers_shipped_files_only_and_refuses_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = asset_dir(tmp.path(), "assets", &ALL_MODULES, false);
    let base = asset_digest(&dir).unwrap();
    write(&dir.join("node_modules/x/index.js"), "ignored");
    write(&dir.join("test/fixture.test.ts"), "ignored");
    assert_eq!(asset_digest(&dir).unwrap(), base);
    write(&dir.join("fixture.json"), "{}");
    let with_fixture = asset_digest(&dir).unwrap();
    assert_ne!(with_fixture, base);
    write(&dir.join("src/host/extra.ts"), "export {};\n");
    assert_ne!(asset_digest(&dir).unwrap(), with_fixture);
    std::os::unix::fs::symlink("/etc/hostname", dir.join("src/linked.ts")).unwrap();
    assert!(asset_digest(&dir).is_err());
}
