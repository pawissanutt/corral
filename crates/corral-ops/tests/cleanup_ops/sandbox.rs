use super::*;

#[test]
fn dropped_run_reaps_only_its_sandbox() {
    run_fixture(false);
}

#[test]
fn dropped_run_reaps_reviewer_sandbox_when_pane_inventory_fails() {
    run_fixture(true);
}

fn run_fixture(fail_inventory: bool) {
    if std::env::var_os("CORRAL_SANDBOX_FIXTURE").is_none() {
        let dir = tempfile::tempdir().unwrap();
        let podman = dir.path().join("podman");
        std::fs::write(
            &podman,
            r##"#!/usr/bin/python3
import os, pathlib, signal, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
if args[0] == 'create':
    marker = next(a.split('=',1)[1] for a in args if a.startswith('corral.run-owner='))
    (root / ('container-' + marker)).write_text('\n'.join(args))
    os.kill(os.getppid(), signal.SIGKILL)
elif args[0] == 'ps':
    filters = [args[i+1][6:] for i, a in enumerate(args[:-1]) if a == '--filter']
    for path in root.glob('container-*'):
        if all(f in path.read_text().splitlines() for f in filters): print(path.name[10:])
elif args[0] == 'rm':
    (root / ('container-' + args[-1])).unlink()
"##,
        )
        .unwrap();
        std::fs::set_permissions(&podman, std::fs::Permissions::from_mode(0o755)).unwrap();
        let foreign = "b".repeat(64);
        std::fs::write(
            dir.path().join(format!("container-{foreign}")),
            format!("corral.load-sandbox=true\ncorral.run-owner={foreign}\n"),
        )
        .unwrap();
        let path = format!(
            "{}:{}",
            dir.path().display(),
            std::env::var("PATH").unwrap()
        );
        let name = if fail_inventory {
            "lifecycle::sandbox::dropped_run_reaps_reviewer_sandbox_when_pane_inventory_fails"
        } else {
            "lifecycle::sandbox::dropped_run_reaps_only_its_sandbox"
        };
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("CORRAL_SANDBOX_FIXTURE", dir.path())
            .env("PATH", path)
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(dir.path().join(format!("container-{foreign}")).exists());
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("container-"))
                .count(),
            1
        );
        return;
    }
    let fx = Fx::new();
    if fail_inventory {
        fx.review();
    }
    let dir = PathBuf::from(std::env::var_os("CORRAL_SANDBOX_FIXTURE").unwrap());
    let owner_dir = fx
        .ctx
        .store
        .scratch_path("tmp")
        .unwrap()
        .join("launch-exits");
    let mut markers =
        vec![std::fs::read_to_string(owner_dir.join(format!("{}-R1.owner", fx.id))).unwrap()];
    if fail_inventory {
        let child = fx.run().review_runs.last().unwrap().id.clone();
        markers.push(
            std::fs::read_to_string(owner_dir.join(format!("{}-R1-{child}.owner", fx.id))).unwrap(),
        );
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/load-sandbox.sh");
    for marker in &markers {
        let status = std::process::Command::new(&script)
            .current_dir(&fx.app)
            .env("CORRAL_PROCESS_OWNER", marker)
            .args(["--", "true"])
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(dir.join(format!("container-{marker}")).exists());
    }
    if fail_inventory {
        let mut state = fx.state();
        state["fail"]
            .as_array_mut()
            .unwrap()
            .push(json!("pane list"));
        std::fs::write(&fx.state, state.to_string()).unwrap();
    }
    corral_ops::move_task(&fx.ctx, &fx.id, Status::Dropped, false).unwrap();
    for marker in markers {
        assert!(!dir.join(format!("container-{marker}")).exists());
    }
    if fail_inventory {
        assert!(
            fx.ctx
                .store
                .events()
                .unwrap()
                .iter()
                .any(|e| e.kind == "cleanup_failed" && e.detail["resource"] == "inventory")
        );
    }
}
