use super::*;

#[test]
fn dropped_run_reaps_only_its_sandbox() {
    // Isolate PATH without changing the environment of parallel tests.
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
    (root / 'labels').write_text('\n'.join(args))
    os.kill(os.getppid(), signal.SIGKILL)
elif args[0] == 'ps':
    labels = (root / 'labels').read_text().splitlines() if (root / 'labels').exists() else []
    filters = [args[i+1][6:] for i, a in enumerate(args[:-1]) if a == '--filter']
    if all(f in labels for f in filters): print('123456789abc')
elif args[0] == 'rm':
    assert args[-1] == '123456789abc'
    (root / 'labels').unlink()
"##,
        )
        .unwrap();
        std::fs::set_permissions(&podman, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.path().join("foreign"), "unrelated container").unwrap();
        let path = format!(
            "{}:{}",
            dir.path().display(),
            std::env::var("PATH").unwrap()
        );
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lifecycle::sandbox::dropped_run_reaps_only_its_sandbox",
                "--nocapture",
            ])
            .env("CORRAL_SANDBOX_FIXTURE", dir.path())
            .env("PATH", path)
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!dir.path().join("labels").exists());
        assert!(dir.path().join("foreign").exists());
        return;
    }
    let fx = Fx::new();
    let owner = fx
        .ctx
        .store
        .state_dir()
        .join("launch-exits")
        .join(format!("{}-R1.owner", fx.id));
    let marker = std::fs::read_to_string(owner).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/load-sandbox.sh");
    let status = std::process::Command::new(script)
        .current_dir(&fx.app)
        .env("CORRAL_PROCESS_OWNER", marker)
        .args(["--", "true"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success()); // Simulate the worker being killed during create.
    let dir = PathBuf::from(std::env::var_os("CORRAL_SANDBOX_FIXTURE").unwrap());
    assert!(dir.join("labels").exists());
    corral_ops::move_task(&fx.ctx, &fx.id, Status::Dropped, false).unwrap();
    assert!(!dir.join("labels").exists());
    assert!(dir.join("foreign").exists());
}
