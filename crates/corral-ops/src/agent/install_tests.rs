use corral_core::agent::AgentRole;

use super::*;
use crate::agent::hooks::preflight;

pub(crate) fn package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/corral-pi")
}

/// A stand-in `bun` that reports `version` and, for `install`, records its argv in
/// `node_modules/` and exits with `install_exit`. A child process writes the executable,
/// so no write descriptor of this multi-threaded test process can make its exec fail
/// with ETXTBSY.
pub(crate) fn fake_bun(dir: &Path, version: &str, install_exit: i32) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let bun = dir.join("bun");
    let source = dir.join("bun.sh");
    std::fs::write(
        &source,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo {version} ;;\ninstall) mkdir -p \
             node_modules && echo \"$*\" > node_modules/.fake-install; exit {install_exit} ;;\n\
             *) exit 9 ;;\nesac\n"
        ),
    )
    .unwrap();
    let installed = std::process::Command::new("install")
        .arg("-m")
        .arg("0755")
        .arg(&source)
        .arg(&bun)
        .status()
        .unwrap();
    assert!(installed.success());
    bun
}

fn digests(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join("assets"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|name| !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_data_root_follows_xdg_then_home() {
    let root = |xdg: Option<&str>, home: Option<&str>| {
        data_root_from(xdg.map(Into::into), home.map(Into::into))
    };
    assert_eq!(
        root(Some("/data"), Some("/home/u")).unwrap(),
        Path::new("/data/corral/corral-pi")
    );
    assert_eq!(
        root(Some("rel"), Some("/home/u")).unwrap(),
        Path::new("/home/u/.local/share/corral/corral-pi")
    );
    assert!(root(None, None).is_err());
}

#[test]
fn legacy_pi_native_install_is_read_in_place_without_moving() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = tmp.path().join("corral/pi-native");
    std::fs::create_dir_all(legacy.join("assets")).unwrap();
    std::fs::write(legacy.join("installed.json"), "retained").unwrap();
    let selected = data_root_from(Some(tmp.path().as_os_str().into()), None).unwrap();
    assert_eq!(selected, legacy);
    assert_eq!(
        std::fs::read_to_string(selected.join("installed.json")).unwrap(),
        "retained"
    );
    assert!(legacy.join("assets").is_dir());
    assert!(!tmp.path().join("corral/corral-pi").exists());
}

#[test]
fn staging_includes_declared_dependency_patches() {
    let source = package();
    let staging = tempfile::tempdir().unwrap();
    copy_shipped(&source, staging.path()).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(source.join("package.json")).unwrap()).unwrap();
    let patches = manifest["patchedDependencies"].as_object().unwrap();
    assert!(!patches.is_empty());
    let digest = asset_digest(staging.path()).unwrap();
    for patch in patches.values() {
        let path = patch.as_str().unwrap();
        assert_eq!(
            std::fs::read(staging.path().join(path))
                .unwrap_or_else(|error| panic!("staged patch {path}: {error}")),
            std::fs::read(source.join(path)).unwrap(),
            "staged patch {path} differs from source"
        );
    }
    let patch = patches.values().next().unwrap().as_str().unwrap();
    std::fs::write(staging.path().join(patch), "changed patch").unwrap();
    assert_ne!(asset_digest(staging.path()).unwrap(), digest);
}

#[test]
fn install_ships_only_host_files_and_dispatch_refuses_until_each_role_lands() {
    // Given the real package and a Bun inside its GO range:
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let bun = fake_bun(tmp.path(), "1.4.2", 0);

    // When it is installed:
    let record = install_into(&package(), &root, Some(bun.clone())).unwrap();

    // Then the copy is named by its digest and holds only what a host runs:
    let dir = root.join("assets").join(&record.digest);
    assert_eq!(record.path, dir);
    assert_eq!(asset_digest(&dir).unwrap(), record.digest);
    for shipped in [
        "package.json",
        "bun.lock",
        "compat.json",
        "src/host/main.ts",
        "src/auth/index.ts",
        "src/auth/login.ts",
    ] {
        assert!(dir.join(shipped).is_file(), "{shipped} missing");
    }
    for absent in ["test", "fixture.json", "src/auth/auth.test.ts"] {
        assert!(!dir.join(absent).exists(), "{absent} was installed");
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("node_modules/.fake-install")).unwrap(),
        "install --frozen-lockfile\n"
    );
    let recorded = std::fs::read_to_string(root.join(RECORD_FILE)).unwrap();
    let checkout = package().canonicalize().unwrap();
    assert!(!recorded.contains(checkout.to_str().unwrap()), "{recorded}");
    assert_eq!(
        (record.bun.as_str(), record.pi.as_str()),
        ("1.4.2", "1.3.0")
    );

    // And dispatch resolves all three landed production roles:
    assert_eq!(installed_in(&root).unwrap(), Some(dir.clone()));
    preflight::assets(&dir, AgentRole::Worker, false).unwrap();
    preflight::assets(&dir, AgentRole::Reviewer, false).unwrap();
    preflight::assets(&dir, AgentRole::Pm, false).unwrap();

    // And a reinstall of the same package is the same installation:
    let again = install_into(&package(), &root, Some(bun)).unwrap();
    assert_eq!(again.digest, record.digest);
    assert_eq!(digests(&root), vec![record.digest.clone()]);

    // A genuinely missing PM role still names its owner and is refused.
    std::fs::remove_file(dir.join("src/roles/pm/index.ts")).unwrap();
    let refusal = preflight::assets(&dir, AgentRole::Pm, false).unwrap_err();
    assert_eq!(refusal.code, "role_unavailable");
    assert!(refusal.detail.starts_with("pm, owner pm-role"), "{refusal}");
}

#[test]
fn a_changed_installation_is_refused_rather_than_dispatched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let record = install_into(&package(), &root, Some(fake_bun(tmp.path(), "1.4.2", 0))).unwrap();
    std::fs::write(
        record.path.join("src/auth/index.ts"),
        "export default {};\n",
    )
    .unwrap();
    let error = installed_in(&root).unwrap_err().to_string();
    assert!(error.starts_with("installed_assets_changed"), "{error}");
}

#[test]
fn install_refuses_fixtures_unsupported_bun_and_a_failed_bun_install_leaving_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let refused = |source: &Path, bun: Option<PathBuf>| {
        install_into(source, &root, bun).unwrap_err().to_string()
    };
    let fixture = tmp.path().join("fixture");
    std::fs::create_dir_all(&fixture).unwrap();
    std::fs::write(fixture.join("fixture.json"), "{}").unwrap();
    assert!(refused(&fixture, None).starts_with("fixture_assets_not_allowed"));
    assert!(refused(&package(), None).starts_with("bun_unavailable"));
    let old = fake_bun(&tmp.path().join("old"), "1.3.0", 0);
    assert!(refused(&package(), Some(old)).starts_with("bun_version_unsupported"));
    let failing = fake_bun(&tmp.path().join("failing"), "1.4.2", 3);
    assert!(refused(&package(), Some(failing)).contains("bun install --frozen-lockfile"));
    assert!(digests(&root).is_empty());
    assert!(read_record(&root).unwrap().is_none());
}

/// A stand-in compiled host: `--version` and `--write-install <dir>` as src/host/compiled.ts
/// answers them, writing the package's real compat.json.
fn fake_host(dir: &Path, version: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let source = dir.join("host.sh");
    let compat = package().join("compat.json");
    std::fs::write(
        &source,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) echo '{version}' ;;\n--write-install) mkdir -p \
             \"$2/assets\" && cp '{}' \"$2/compat.json\" && echo contract > \"$2/assets/worker-contract.md\" ;;\n\
             *) exit 9 ;;\nesac\n",
            compat.display()
        ),
    )
    .unwrap();
    let host = dir.join(HOST_BINARY);
    let installed = std::process::Command::new("install")
        .args(["-m", "0755"])
        .arg(&source)
        .arg(&host)
        .status()
        .unwrap();
    assert!(installed.success());
    host
}

#[test]
fn a_compiled_host_installs_without_bun_and_serves_every_role() {
    // Given a compiled host and no Bun anywhere:
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let host = fake_host(
        &tmp.path().join("release"),
        "corral-pi-host pi 1.3.0 bun 1.4.2",
    );

    // When it is installed:
    let record = install_compiled(&host, &root).unwrap();

    // Then the installation is the executable plus what it wrote, named by its digest:
    let dir = root.join("assets").join(&record.digest);
    assert_eq!(record.path, dir);
    assert_eq!(
        (record.bun.as_str(), record.pi.as_str()),
        ("1.4.2", "1.3.0")
    );
    assert!(dir.join(HOST_BINARY).is_file() && dir.join("compat.json").is_file());
    assert!(!dir.join("src/host/main.ts").exists() && !dir.join("node_modules").exists());
    assert_eq!(installed_in(&root).unwrap(), Some(dir.clone()));

    // And every role is served by the bundle, with no source modules on disk:
    for role in [AgentRole::Worker, AgentRole::Reviewer, AgentRole::Pm] {
        preflight::assets(&dir, role, false).unwrap();
    }

    // And reinstalling the same executable is the same installation:
    assert_eq!(
        install_compiled(&host, &root).unwrap().digest,
        record.digest
    );
    assert_eq!(digests(&root), vec![record.digest.clone()]);

    // And a replaced executable is refused rather than launched:
    std::fs::write(dir.join(HOST_BINARY), "#!/bin/sh\nexit 0\n").unwrap();
    assert!(
        installed_in(&root)
            .unwrap_err()
            .to_string()
            .starts_with("installed_assets_changed")
    );
}

#[test]
fn source_rebuild_replaces_stale_host_and_rejects_changed_head() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    let source = checkout.join("packages/corral-pi");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(checkout.join("scripts")).unwrap();
    std::fs::write(source.join("compat.json"), "{}").unwrap();
    let new = fake_host(&tmp.path().join("new"), "corral-pi-host pi 2.0 bun 1.4.2");
    let script = checkout.join("scripts/build-pi-host.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncp '{}' \"$1\"\nchmod +x \"$1\"\n",
            new.display()
        ),
    )
    .unwrap();
    set_executable(&script).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "Test"],
        &["add", "."],
        &["commit", "-qm", "fixture"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&checkout)
                .stdin(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        );
    }
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&checkout)
        .output()
        .unwrap();
    let sha = String::from_utf8(sha.stdout).unwrap();
    let sibling = fake_host(&tmp.path().join("bin"), "corral-pi-host pi 1.0 bun 1.4.2");
    let root = tmp.path().join("data");
    let old = install_compiled(&sibling, &root).unwrap();
    let fresh = install_default(&root, Some(&source), &sibling, false, Some(sha.trim())).unwrap();
    assert_ne!(old.digest, fresh.digest);
    let before = std::fs::read(&sibling).unwrap();
    let record = std::fs::read(root.join(RECORD_FILE)).unwrap();
    let error = install_default(&root, Some(&source), &sibling, false, Some("wrong-build"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(sha.trim()) && error.contains("wrong-build") && error.contains("--from"),
        "{error}"
    );
    assert_eq!(before, std::fs::read(&sibling).unwrap());
    assert_eq!(record, std::fs::read(root.join(RECORD_FILE)).unwrap());
    assert_eq!(
        install_default(&root, None, &sibling, false, None)
            .unwrap()
            .digest,
        fresh.digest
    );
    assert_eq!(
        install_default(&root, Some(&source), &sibling, true, Some("wrong-build"))
            .unwrap()
            .digest,
        fresh.digest
    );
}

#[test]
fn a_host_that_does_not_identify_itself_is_refused_leaving_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let host = fake_host(&tmp.path().join("release"), "something else");
    let error = install_compiled(&host, &root).unwrap_err().to_string();
    assert!(
        error.contains("not `corral-pi-host pi <v> bun <v>`"),
        "{error}"
    );
    assert!(digests(&root).is_empty());
    assert!(read_record(&root).unwrap().is_none());
    assert_eq!(
        parse_host_version("corral-pi-host pi 1.3.0 bun 1.4.2"),
        Some(("1.3.0".to_owned(), "1.4.2".to_owned()))
    );
}
