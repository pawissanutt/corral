use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::agent::{ContainerRef, pm_state};
use corral_core::inbox::Recipient;
use serde_json::json;

use super::*;
use crate::agent::BindTarget;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc};
use crate::agent::hooks::pm::tests::fixture;
use crate::agent::launch::{BOOTSTRAP_FILE, CTL_CAP_FILE, HOST_ENTRY, generation_dir};

const PANE: &str = "w1:p9";

fn owner_pane(fx: &Fx) {
    std::fs::write(fx.root().join(PANE), "").unwrap();
}

fn pm_target() -> BindTarget {
    BindTarget::Pm {
        project: "corral".into(),
    }
}

fn code(error: &anyhow::Error) -> Option<&'static str> {
    error
        .downcast_ref::<PiRefusal>()
        .map(|refusal| refusal.code)
}

#[test]
fn bound_mode_refuses_outside_herdr() {
    for value in [None, Some("")] {
        let error = current_pane(value).unwrap_err();
        assert_eq!(code(&error), Some("not_in_herdr"), "{error}");
        assert!(error.to_string().contains("--bare"), "{error}");
    }
    assert_eq!(current_pane(Some(PANE)).unwrap(), PANE);
}

#[test]
fn bound_mode_refuses_a_live_bound_pm_naming_its_pane_and_changes_nothing() {
    // Given a live Pi-native PM bound in its own pane.
    let fx = fixture();
    owner_pane(&fx);
    let host = HostProc::spawn(true);
    fx.bind(&pm_target(), "pm-pane", &host);
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    // When `corral pi` runs in the owner's pane.
    let error = adopt(&f::ctx_at(fx.root()), PANE, &Overrides::default()).unwrap_err();
    // Then it refuses, names the pane and the replace command, and opens no rival generation.
    assert_eq!(code(&error), Some("pm_alive"), "{error}");
    let text = error.to_string();
    assert!(text.contains("pm-pane") && text.contains("corral agent resume --pm --replace"));
    assert_eq!(pm_state::load(&fx.ctx.store).unwrap().unwrap(), before);
    assert!(!fx.ctx.store.state_dir().join("pm.json").exists());
    assert!(fx.calls("pane run").is_empty());
}

#[test]
fn bound_mode_refuses_profile_and_scope_failures_before_any_state() {
    let fx = fixture();
    owner_pane(&fx);
    std::fs::remove_file(fx.root().join("assets/src/roles/pm/index.ts")).unwrap();
    let error = adopt(&f::ctx_at(fx.root()), PANE, &Overrides::default()).unwrap_err();
    assert!(error.to_string().contains("role_unavailable"), "{error}");
    let error = adopt(
        &f::ctx_at(fx.root()),
        PANE,
        &Overrides {
            provider: Some("mistral".into()),
            ..Overrides::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown provider"), "{error}");
    assert!(pm_state::load(&fx.ctx.store).unwrap().is_none());
}

#[test]
fn a_fresh_store_adopts_the_owner_pane_as_generation_one() {
    let fx = fixture();
    owner_pane(&fx);
    let prepared = adopt(&f::ctx_at(fx.root()), PANE, &Overrides::default()).unwrap();
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!((pm.pane.as_str(), pm.state.binding.generation), (PANE, 1));
    assert!(pm.adopted());
    assert_eq!(pm.state.generation(1).unwrap().pane, PANE);
    let dir = generation_dir(&fx.ctx.store, &Recipient::Pm, 1).unwrap();
    assert_eq!(prepared.bootstrap, dir.join(BOOTSTRAP_FILE));
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fx.ctx.store.state_dir().join("pm.json")).unwrap())
            .unwrap();
    assert_eq!(saved["pane_id"], PANE);
    assert!(fx.ctx.store.prompts_dir().join("pm.md").is_file());
    assert!(fx.calls("pane run").is_empty());
    // An adopted pane whose host is not running is never the PM's, and never closed.
    let ctx = f::ctx_at(fx.root());
    assert_eq!(crate::pm::live_pm_pane(&ctx).unwrap(), None);
    assert!(fx.calls("pane close").is_empty());
}

#[test]
fn a_dead_pm_is_taken_over_with_resume_fencing_and_its_recovery_never_types_into_the_pane() {
    // Given a PM bound in pane pm-pane whose host died.
    let fx = fixture();
    owner_pane(&fx);
    let mut old = HostProc::spawn(true);
    let first = fx.bind(&pm_target(), "pm-pane", &old);
    old.kill();
    // When `corral pi` runs in the owner's pane.
    adopt(&f::ctx_at(fx.root()), PANE, &Overrides::default()).unwrap();
    // Then generation 1 is fenced, generation 2 opened here, a notice posted and nothing typed.
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert!(pm.state.generation(1).unwrap().fenced.is_some());
    assert_eq!(pm.state.binding.generation, 2);
    assert_eq!(pm.state.generation(2).unwrap().pane, PANE);
    assert!(pm.adopted());
    assert!(!fx.events("agent_resumed").is_empty());
    assert!(fx.calls("pane run").is_empty());
    // And the host started from that bootstrap binds generation 2 to this pane with hello.
    let host = HostProc::spawn(true);
    let mut previous = first.clone();
    previous.pane = PANE.into();
    fx.hello_next(&previous, &Recipient::Pm, &host);
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert!(pm.state.generation(2).unwrap().bound.is_some());
    // Resume replaces a live adopted host only through --replace; once it is gone it refuses.
    let mut host = host;
    host.kill();
    let error =
        crate::agent::recovery::resume_container(&f::ctx_at(fx.root()), &ContainerRef::Pm, false)
            .unwrap_err();
    assert!(error.to_string().starts_with("pane_adopted"), "{error}");
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert!(pm.state.generation(2).unwrap().fenced.is_none());
    assert!(fx.calls("pane run").is_empty());
}

fn approve(fx: &Fx, task: &str) {
    let selection = corral_core::OperatingSelection {
        tasks: [task.to_owned()].into(),
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
        .unwrap();
}

#[test]
fn a_refused_takeover_keeps_the_original_pane_adopted_and_resume_never_types_into_it() {
    // Given a PM adopted in owner pane A whose host died.
    let mut fx = fixture();
    owner_pane(&fx);
    let mut old = HostProc::spawn(true);
    let first = fx.bind(&pm_target(), "pm-pane", &old);
    old.kill();
    adopt(&f::ctx_at(fx.root()), PANE, &Overrides::default()).unwrap();
    let mut host = HostProc::spawn(true);
    let mut previous = first.clone();
    previous.pane = PANE.into();
    fx.hello_next(&previous, &Recipient::Pm, &host);
    host.kill();
    // And an approved scope that now selects a different project.
    fx.ctx.store.config.projects.insert(
        "other".into(),
        serde_json::from_value(json!({"path": "app"})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let other = fx
        .ctx
        .store
        .create(
            "other",
            Some("other"),
            corral_core::Status::Ready,
            "fixture",
        )
        .unwrap();
    approve(&fx, &other.id);
    // When `corral pi` in pane B is refused by recovery.
    std::fs::write(fx.root().join("w1:p8"), "").unwrap();
    let error = adopt(&f::ctx_at(fx.root()), "w1:p8", &Overrides::default()).unwrap_err();
    assert!(error.to_string().starts_with("out_of_scope"), "{error}");
    // Then pane A is still the adopted PM pane.
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!(pm.pane, PANE);
    assert!(pm.adopted(), "{:?}", pm.adopted_pane);
    // And after the original scope returns, resume refuses instead of typing into pane A.
    approve(&fx, &fx.id);
    let error =
        crate::agent::recovery::resume_container(&f::ctx_at(fx.root()), &ContainerRef::Pm, false)
            .unwrap_err();
    assert!(error.to_string().starts_with("pane_adopted"), "{error}");
    assert!(fx.calls("pane run").is_empty());
}

#[test]
fn pm_open_in_a_new_pane_drops_the_adoption() {
    let fx = fixture();
    owner_pane(&fx);
    adopt(&f::ctx_at(fx.root()), PANE, &Overrides::default()).unwrap();
    crate::agent::open_generation(&fx.ctx.store, &pm_target(), "pm-pane").unwrap();
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert!(!pm.adopted() && pm.adopted_pane.is_none());
}

fn fake_bun(dir: &Path) -> std::path::PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("bun");
    std::fs::write(
        &path,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.4.2; exit 0; fi\n\
         out=\"$CORRAL_FAKE_BUN_OUT\"\nprintf '%s\\n' \"$@\" > \"$out/argv\"\n\
         ls -A \"$CORRAL_FAKE_BUN_GEN\" > \"$out/generation\"\n\
         cat \"/proc/self/fd/$4\" > \"$out/message\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    bin
}

#[test]
fn bound_mode_execs_in_process_and_leaves_no_bootstrap_on_disk() {
    let fx = fixture();
    owner_pane(&fx);
    let assets = fx.root().join("assets");
    let compat = json!({"version": 1, "verdict": "GO", "bun": {"range": "1.4.2"}});
    std::fs::write(assets.join("compat.json"), compat.to_string()).unwrap();
    std::fs::create_dir_all(assets.join(HOST_ENTRY).parent().unwrap()).unwrap();
    std::fs::write(assets.join(HOST_ENTRY), "// host\n").unwrap();
    let bin = fake_bun(fx.root());
    let out = fx.root().join("bun-out");
    std::fs::create_dir(&out).unwrap();
    let generation = generation_dir(&fx.ctx.store, &Recipient::Pm, 1).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "agent::pi::tests::bound_exec_child", "--ignored"])
        .args(["--nocapture", "--test-threads", "1"])
        .env("CORRAL_PI_TEST_ROOT", fx.root())
        .env(PANE_ENV, PANE)
        .env("PATH", path)
        .env("CORRAL_FAKE_BUN_OUT", &out)
        .env("CORRAL_FAKE_BUN_GEN", &generation)
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{status}");
    let argv = std::fs::read_to_string(out.join("argv")).unwrap();
    assert!(argv.contains("--launch-fd"), "{argv}");
    let listing = std::fs::read_to_string(out.join("generation")).unwrap();
    assert!(!listing.contains(BOOTSTRAP_FILE), "{listing}");
    assert!(listing.contains(CTL_CAP_FILE), "{listing}");
    assert!(!generation.join(BOOTSTRAP_FILE).exists());
    let message: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("message")).unwrap()).unwrap();
    assert_eq!(
        (message["role"].as_str(), message["pane"].as_str()),
        (Some("pm"), Some(PANE))
    );
}

#[test]
#[ignore = "exec child of bound_mode_execs_in_process_and_leaves_no_bootstrap_on_disk"]
fn bound_exec_child() {
    let root = std::path::PathBuf::from(std::env::var("CORRAL_PI_TEST_ROOT").unwrap());
    let error = bound(&root, &f::ctx_at(&root), &Overrides::default()).unwrap_err();
    panic!("bound returned instead of exec: {error:#}");
}

#[test]
fn bare_refuses_without_an_installation_or_with_an_unknown_provider() {
    let data = tempfile::tempdir().unwrap();
    let args = |provider: Option<&str>| BareArgs {
        model: None,
        provider: provider.map(str::to_owned),
        effort: None,
    };
    let error = bare_command(data.path(), &args(None), data.path(), None).unwrap_err();
    assert_eq!(code(&error), Some("assets_unavailable"), "{error}");
    let error = bare_command(data.path(), &args(Some("mistral")), data.path(), None).unwrap_err();
    assert_eq!(code(&error), Some("usage_error"), "{error}");
    assert!(!data.path().join(".corral").exists());
}

#[test]
fn bare_runs_a_compiled_installation_without_bun_or_source_modules() {
    use crate::agent::bridge::assets::HOST_BINARY;
    use std::os::unix::fs::PermissionsExt;
    // Given a compiled host installed as the managed installation, and a stored Codex login.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let release = tmp.path().join("release");
    std::fs::create_dir_all(&release).unwrap();
    let compat = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/corral-pi/compat.json");
    let host = release.join(HOST_BINARY);
    std::fs::write(
        &host,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 'corral-pi-host pi 1.3.0 bun 1.4.2' ;;\n\
             --write-install) cp '{}' \"$2/compat.json\" ;;\n*) exit 9 ;;\nesac\n",
            compat.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
    crate::agent::install::install_compiled(&host, &root).unwrap();
    let auth = root.join("auth");
    std::fs::create_dir_all(&auth).unwrap();
    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(auth.join("openai-codex.json"), "{}").unwrap();
    std::fs::set_permissions(
        auth.join("openai-codex.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let assets = crate::agent::install::installed_in(&root).unwrap().unwrap();
    assert!(!assets.join(BARE_ENTRY).exists() && !assets.join(AUTH_MODULE).exists());
    // When bare mode prepares its launch with no Bun on PATH.
    let args = BareArgs {
        model: Some("gpt-6.1-sol".into()),
        provider: None,
        effort: None,
    };
    let command = bare_command(&root, &args, tmp.path(), None).unwrap();
    // Then it runs the installed compiled host's bare mode.
    assert_eq!(Path::new(command.get_program()), assets.join(HOST_BINARY));
    let argv: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let cwd = tmp.path().display().to_string();
    let expected = [
        "bare",
        "--cwd",
        &cwd,
        "--provider",
        "openai-codex",
        "--model",
        "gpt-6.1-sol",
    ];
    assert_eq!(argv, expected);
    assert_eq!(command.get_current_dir(), Some(assets.as_path()));
}
