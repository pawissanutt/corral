use std::{
    fs,
    process::{Command, Stdio},
};

use corral_facts::{PackageEdgeKind, ReadStatus, ReferenceBasis, ReferenceKind};
use serde_json::json;

use super::*;

struct Fixture {
    workspace: tempfile::TempDir,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let repo = workspace.path().join("repo");
        fs::create_dir_all(repo.join("app")).unwrap();
        fs::create_dir_all(repo.join("lib")).unwrap();
        let config = Config::parse(
            r#"
            [projects.sample]
            path = "repo"
            [projects.sample.env]
            FIXTURE_ENV = "project value"
            [extensions.fixture]
            projects = ["sample"]
            facts = "sh ../provider.sh"
        "#,
        )
        .unwrap();
        let fixture = Self { workspace, config };
        fixture.git(&["init", "-b", "main"]);
        fs::write(
            repo.join("app/package.json"),
            r#"{"name":"app","dependencies":{"lib":"file:../lib"}}"#,
        )
        .unwrap();
        fs::write(repo.join("lib/package.json"), r#"{"name":"lib"}"#).unwrap();
        fs::write(
            repo.join("app/index.ts"),
            "import '../lib/index';\nexport const app = 1;\n",
        )
        .unwrap();
        fs::write(repo.join("lib/index.ts"), "export const lib = 1;\n").unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "fixture"]);
        fixture
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(self.workspace.path().join("repo"))
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
            ])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn script(&self, body: &str) {
        fs::write(
            self.workspace.path().join("provider.sh"),
            format!(
                r#"
set -eu
[ "$(cd "$CORRAL_NEXT_ROOT" && pwd -P)" = "$(cd .. && pwd -P)" ]
[ "$CORRAL_PROJECT" = sample ]
[ "$CORRAL_EXTENSION" = fixture ]
[ "$CORRAL_ACTOR" = extension ]
[ "$FIXTURE_ENV" = 'project value' ]
cat > ../request.json
{body}
"#
            ),
        )
        .unwrap();
    }

    fn request(&self) -> Value {
        serde_json::from_slice(&fs::read(self.workspace.path().join("request.json")).unwrap())
            .unwrap()
    }

    fn reader(&self) -> FactsReader<'_> {
        FactsReader::new(self.workspace.path(), &self.config, "sample").unwrap()
    }
}

#[test]
fn slot_selects_only_one_enabled_provider_in_scope() {
    let mut config = Config::parse(
        r#"
        [projects.a]
        [projects.b]
        [extensions.for_a]
        projects = ["a"]
        facts = "first"
        [extensions.disabled]
        enabled = false
        facts = "second"
        [extensions.tool_only.tools.context]
        run = "third"
    "#,
    )
    .unwrap();
    assert_eq!(
        config.extension_for_slot("a", Slot::Facts),
        Some(("for_a", "first"))
    );
    assert_eq!(config.extension_for_slot("b", Slot::Facts), None);
    config.extensions.get_mut("disabled").unwrap().enabled = true;
    assert_eq!(config.extension_for_slot("a", Slot::Facts), None);
    assert_eq!(
        config.extension_for_slot("b", Slot::Facts),
        Some(("disabled", "second"))
    );
}

#[test]
fn definitions_enrich_builtin_facts_and_cache_by_tree() {
    let fixture = Fixture::new();
    let reader = fixture.reader();
    let revision = reader.resolve_revision("HEAD").unwrap();
    let builtin = reader.builtin_snapshot(&revision).unwrap();
    assert_eq!(builtin.structure.file_references.len(), 1);
    assert_eq!(
        builtin.structure.file_references[0].basis,
        ReferenceBasis::Syntax
    );
    // The file exists in the checkout but not in the requested commit.
    fs::write(reader.repo.join("untracked.ts"), "").unwrap();
    fixture.script(
        r#"
cat <<'ROWS'
{"type":"definition","path":"app/index.ts","name":"app","line":2,"end_line":2,"kind":"constant"}
{"type":"definition","path":"untracked.ts","name":"ghost","line":1,"end_line":1,"kind":"constant"}
{"type":"done","provided":["definitions"],"skipped":{"unsupported":1},"omitted":{"limit":2}}
ROWS
"#,
    );
    let mut facts = builtin.clone();
    let report = reader.enrich_snapshot(&mut facts).unwrap();
    assert_eq!(
        fixture.request(),
        json!({"v":1,"query":"snapshot","repo":reader.repo,"rev":revision.commit(),"want":SNAPSHOT_WANT})
    );
    assert_eq!(report.extension, "fixture");
    assert_eq!(report.error, None);
    assert_eq!(report.coverage.provided, ["definitions"]);
    assert_eq!(report.coverage.not_provided, ["files", "references"]);
    assert_eq!(report.coverage.dropped_unknown_files, 1);
    assert_eq!(report.coverage.skipped["unsupported"], 1);
    assert_eq!(report.coverage.omitted["limit"], 2);
    let enriched = facts
        .structure
        .files
        .iter_mut()
        .find(|file| file.path == "app/index.ts")
        .unwrap();
    assert_eq!(enriched.definitions.len(), 1);
    assert_eq!(enriched.definitions[0].name, "app");
    let definitions = std::mem::take(&mut enriched.definitions);
    assert_eq!(facts, builtin);

    fixture.git(&["commit", "--allow-empty", "-m", "same tree"]);
    let later = reader.resolve_revision("HEAD").unwrap();
    assert_ne!(revision.commit(), later.commit());
    fixture.script("exit 77");
    let mut facts = reader.builtin_snapshot(&later).unwrap();
    assert_eq!(reader.enrich_snapshot(&mut facts).unwrap().error, None);
    assert_eq!(
        facts
            .structure
            .files
            .iter()
            .find(|f| f.path == "app/index.ts")
            .unwrap()
            .definitions,
        definitions
    );
}

#[test]
fn reference_replacement_preserves_declarations_and_delta_is_requested_separately() {
    let fixture = Fixture::new();
    let reader = fixture.reader();
    let base = reader.resolve_revision("HEAD").unwrap();
    fixture.script(
        r#"
cat <<'ROWS'
{"type":"file","path":"app/index.ts","read":"failed"}
{"type":"reference","from":"lib/index.ts","to":"app/index.ts","kind":"call","count":3}
{"type":"reference","from":"app/index.ts","to":"missing.ts","kind":"import","count":4}
{"type":"done","provided":["files","references"],"skipped":{},"omitted":{}}
ROWS
"#,
    );
    let mut facts = reader.builtin_snapshot(&base).unwrap();
    let builtin = facts.structure.clone();
    assert_eq!(builtin.package_edges.len(), 1);
    assert_eq!(
        builtin.package_edges[0].kind,
        PackageEdgeKind::DeclaredAndReferenced
    );
    let report = reader.enrich_snapshot(&mut facts).unwrap();
    assert_eq!(report.error, None);
    assert_eq!(report.coverage.dropped_unknown_files, 1);
    assert_eq!(facts.structure.files[0].read_status, ReadStatus::Failed);
    assert_eq!(facts.structure.coverage.failed, builtin.coverage.failed + 1);
    let refs = &facts.structure.file_references;
    assert_eq!(refs.len(), 1);
    assert_eq!(
        (
            &*refs[0].from,
            &*refs[0].to,
            refs[0].kind,
            refs[0].basis,
            refs[0].count
        ),
        (
            "lib/index.ts",
            "app/index.ts",
            ReferenceKind::Call,
            ReferenceBasis::Resolved,
            3
        )
    );
    let module_refs = &facts.structure.module_references;
    assert_eq!(module_refs.len(), 1);
    assert_eq!(module_refs[0].count, 3);
    assert_eq!(module_refs[0].basis, ReferenceBasis::Resolved);
    let edges = &facts.structure.package_edges;
    assert_eq!(edges.len(), 2);
    assert_eq!(
        (edges[0].kind, edges[0].references),
        (PackageEdgeKind::DeclaredOnly, 0)
    );
    assert_eq!(
        (edges[1].kind, edges[1].references),
        (PackageEdgeKind::ReferencedOnly, 3)
    );
    assert_eq!(facts.structure.dependencies, builtin.dependencies);

    // Same revision is still a different query/cache entry.
    fixture.script(
        r#"
cat <<'ROWS'
{"type":"test","path":"app/index.ts","name":"candidate"}
{"type":"dependent","path":"app/index.ts","name":"app","line":2,"of":"lib"}
{"type":"done","provided":["tests","dependents"],"skipped":{},"omitted":{}}
ROWS
"#,
    );
    let mut delta = reader.builtin_delta(&base, &base).unwrap();
    assert_eq!(
        reader.enrich_delta(&base, &base, &mut delta).unwrap().error,
        None
    );
    assert_eq!(
        fixture.request(),
        json!({"v":1,"query":"delta","repo":reader.repo,"base":base.commit(),"head":base.commit(),"want":DELTA_WANT})
    );
    assert_eq!(delta.changes, None);
    assert_eq!(delta.tests.unwrap()[0].name, "candidate");
    assert_eq!(delta.dependents.unwrap()[0].of, "lib");
}

#[test]
fn invalid_provider_streams_leave_builtin_facts_untouched() {
    let fixture = Fixture::new();
    let mut reader = fixture.reader();
    let revision = reader.resolve_revision("HEAD").unwrap();
    let builtin = reader.builtin_snapshot(&revision).unwrap();
    for (tail, timeout, max_stdout, error) in [
        (
            "echo 'fixture exploded' >&2; exit 7",
            TIMEOUT,
            MAX_STDOUT,
            "fixture exploded",
        ),
        (":", TIMEOUT, MAX_STDOUT, "done"),
        ("echo invalid", TIMEOUT, MAX_STDOUT, "line 2"),
        (
            "exec sleep 10",
            Duration::from_millis(50),
            MAX_STDOUT,
            "timed out",
        ),
        (":", TIMEOUT, 10, "stdout exceeded 10 bytes"),
    ] {
        fixture.script(&format!(r#"
printf '%s\n' '{{"type":"definition","path":"app/index.ts","name":"app","line":2,"end_line":2,"kind":"constant"}}'
{tail}
"#));
        reader.timeout = timeout;
        reader.max_stdout = max_stdout;
        let mut facts = builtin.clone();
        let report = reader.enrich_snapshot(&mut facts).unwrap();
        assert!(
            report
                .error
                .as_deref()
                .is_some_and(|message| message.contains(error)),
            "{report:?}"
        );
        assert_eq!(report.coverage.not_provided, SNAPSHOT_WANT);
        assert_eq!(facts, builtin);
    }
}
