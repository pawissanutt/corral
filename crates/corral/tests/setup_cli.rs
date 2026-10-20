//! Setup runs only against private files and fake local harness observations.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::ProfileSetupProposal;
use serde_json::{Value, json};

const NOW: &str = "2026-10-18T12:00:00Z";
const CONFIG: &str = r#"
[harnesses.fixture]
command = "fixture-harness {model_args} {prompt_file}"
flags = "omo"

[profiles.existing]
harness = "fixture"

[pm]
agent = "existing"

[artifacts]
max_bytes = 16384
"#;

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            temp: tempfile::tempdir().unwrap(),
        };
        for dir in [
            ".corral", "home", "config", "data", "state", "cache", "runtime", "bin",
        ] {
            std::fs::create_dir(fixture.root().join(dir)).unwrap();
        }
        std::fs::write(fixture.config(), CONFIG).unwrap();
        fixture
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn config(&self) -> PathBuf {
        self.root().join(".corral/corral.toml")
    }

    fn cli(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_corral"))
            .env_clear()
            .env("HOME", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("XDG_RUNTIME_DIR", self.root().join("runtime"))
            .env("PATH", self.root().join("bin"))
            .current_dir(self.root())
            .arg("--root")
            .arg(self.root())
            .arg("setup")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            !self.root().join("herdr-contacted").exists(),
            "setup must never invoke herdr"
        );
        output
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{args:?}: {:?}", output.stderr);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn reject(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {:?}", output.stdout);
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"].is_string(), "{error}");
        error
    }

    fn write(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.root().join(name);
        std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
    }

    fn input(&self) -> Value {
        let revision = self.ok(&["status"])["config_revision"].clone();
        let evidence = json!({
            "version":1, "id":"local-model", "retrieved_at":NOW,
            "published_on":null, "measured_on":null,
            "source":{"kind":"local_discovery","method":"fixture models"},
            "claim":"Exact local model listing", "finding":"Owner connection reported",
            "limitations":["Execution and comparative quality are untested"]
        });
        let facets = json!({
            "installed":true,"discovered":true,"user_reported":true,
            "access_verified":false,"execution_tested":false
        });
        json!({
            "proposal":{
                "version":1,"id":"proposal-1","prepared_at":NOW,
                "procedure_version":"fixture-v1","based_on_config_revision":revision,
                "preferences":{}, "basis":{"kind":"provisional","exception":"No comparison source"},
                "profiles":{"custom.database":{
                    "harness":"fixture","model":"openai/exact-fixture-v1",
                    "vendor":"openai","effort":"high","args":[],
                    "metadata":{
                        "display_name":"Database","purpose":"Persistence correctness",
                        "guidance":{"use_when":["Persistence work"],"avoid_when":["Prose only"],
                            "positive_examples":["Repair rollback"],"negative_examples":["Edit docs"],
                            "distinctions":["Writing handles prose"]},
                        "instructions":"Check transaction boundaries",
                        "eligibility":{"enabled":true,"automatic":true,"roles":["planning"],
                            "projects":[]},
                        "revision":"profile-r1","evidence":"local-model",
                        "candidates":[{
                            "id":"preferred","harness":"fixture","connection":"fixture-owner",
                            "provider":"openai","model":"openai/exact-fixture-v1",
                            "effort":"high","args":[],"evidence":"local-model",
                            "cost":{"kind":"subscription"}
                        }],"fallback":null
                    }
                }},
                "roles":{"planning":"custom.database"},
                "evidence":{"local-model":evidence},
                "recommendations":{"custom.database":{
                    "reason":"Owner local fixture selection","evidence":["local-model"],
                    "alternatives_considered":[],"uncertainty":["Comparative quality untested"]
                }},
                "access":[{"profile":"custom.database","candidate":"preferred",
                    "observed_at":NOW,"facets":facets,"evidence":["local-model"]}],
                "repair_policy":null
            },
            "discovery":{
                "observed_at":NOW,
                "harnesses":[{"key":"fixture","installed":true,
                    "models":["openai/exact-fixture-v1"],"efforts":["high"]}],
                "candidates":[{"harness":"fixture","connection":"fixture-owner",
                    "provider":"openai","model":"openai/exact-fixture-v1",
                    "observed_at":NOW,"facets":facets,
                    "settings":[{"effort":"high","args":[]}],"evidence":[evidence]}]
            }
        })
    }

    fn proposed(&self) -> (Value, PathBuf) {
        let input = self.input();
        self.ok(&["begin", "--expected-revision", "0"]);
        let file = self.write("proposal.json", &input);
        let record = self.ok(&[
            "propose",
            "--file",
            file.to_str().unwrap(),
            "--expected-revision",
            "1",
        ]);
        assert_eq!(record["revision"], 2);
        assert_eq!(record["state"], "proposed");
        (input, file)
    }

    fn approval(&self, input: &Value) -> Value {
        let proposal: ProfileSetupProposal =
            serde_json::from_value(input["proposal"].clone()).unwrap();
        json!({
            "version":1,"id":"approval-1","by":"owner","at":NOW,
            "proposal":proposal.id,"proposal_digest":proposal.digest().unwrap(),
            "based_on_config_revision":proposal.based_on_config_revision,
            "acknowledged_evidence_gap":true
        })
    }
}

#[test]
fn interview_answers_merge_and_resume_across_cli_processes() {
    // Given non-secret interview answers persisted by independent CLI invocations.
    let fixture = Fixture::new();
    fixture.ok(&["begin", "--expected-revision", "0"]);
    let access = fixture.write("access.json", &json!({"access":"One existing local model"}));
    fixture.ok(&[
        "answers",
        "--file",
        access.to_str().unwrap(),
        "--expected-revision",
        "1",
    ]);
    let preference = fixture.write(
        "preference.json",
        &json!({"quality_cost_preference":"quality"}),
    );
    fixture.ok(&[
        "answers",
        "--file",
        preference.to_str().unwrap(),
        "--expected-revision",
        "2",
    ]);
    let before = snapshot(fixture.root());

    // When a later process reopens the interview.
    let resumed = fixture.ok(&["begin", "--expected-revision", "3"]);

    // Then answers merge, revision is stable, and resuming writes nothing.
    assert_eq!(resumed["revision"], 3);
    assert_eq!(
        resumed["answers"],
        json!({
            "access":"One existing local model", "quality_cost_preference":"quality"
        })
    );
    assert_eq!(before, snapshot(fixture.root()));
    fixture.reject(&[
        "answers",
        "--file",
        access.to_str().unwrap(),
        "--expected-revision",
        "1",
    ]);
}

#[test]
fn bootstrap_preview_needs_no_profiles_and_never_contacts_engine() {
    // Given one configured harness but no profiles or resolved PM assignment.
    let fixture = Fixture::new();
    std::fs::write(
        fixture.config(),
        r#"
[harnesses.fixture]
command = "fixture-harness {model_args} {prompt_file}"
flags = "omo"
[agents.fixture]
command = "fixture-harness {model_args} {prompt_file}"
[pm]
agent = "fixture"
"#,
    )
    .unwrap();
    let before = snapshot(fixture.root());

    // When previewing a connection before owner confirmation.
    let preview = fixture.ok(&[
        "bootstrap",
        "--harness",
        "fixture",
        "--model",
        "local/only-model",
    ]);

    // Then nothing launches or changes and access is explicitly unverified.
    assert_eq!(preview["connection"]["model"], "local/only-model");
    assert_eq!(preview["confirmed"], false);
    assert_eq!(preview["access_verified"], false);
    assert_eq!(preview["execution_tested"], false);
    assert_eq!(before, snapshot(fixture.root()));
}

#[test]
fn procedure_is_available_without_workspace_or_source_checkout() {
    // Given no workspace configuration.
    let fixture = Fixture::new();
    std::fs::remove_dir_all(fixture.root().join(".corral")).unwrap();

    // When reading the procedure from the binary.
    let procedure = fixture.ok(&["procedure"]);

    // Then the machine identity and shipped source bytes match.
    assert_eq!(procedure["version"], corral_ops::MODEL_SELECTION_VERSION);
    assert_eq!(
        procedure["procedure"],
        corral_ops::MODEL_SELECTION_PROCEDURE
    );
}

#[test]
fn filling_only_template_placeholders_passes_validate() {
    // Given the template printed by the real CLI.
    let fixture = Fixture::new();
    let revision = fixture.ok(&["status"])["config_revision"].clone();
    let template = fixture.cli(&["template"]);
    assert!(template.status.success());
    let mut text = String::from_utf8(template.stdout).unwrap();
    assert_eq!(text, corral_ops::SETUP_TEMPLATE);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        parsed["proposal"]["procedure_version"],
        corral_ops::MODEL_SELECTION_VERSION
    );

    // When only its placeholders are replaced.
    let values = [
        ("<proposal-id>", "proposal-1"),
        ("<prepared-at>", NOW),
        ("<observed-at>", NOW),
        ("<retrieved-at>", NOW),
        ("<config-revision>", revision.as_str().unwrap()),
        ("<profile-key>", "custom.database"),
        ("<harness>", "fixture"),
        ("<model>", "openai/exact-fixture-v1"),
        ("<provider>", "openai"),
        ("<effort>", "high"),
        ("<connection>", "fixture-owner"),
        ("<candidate-id>", "preferred"),
        ("<evidence-id>", "local-model"),
        ("<discovery-method>", "fixture models"),
        ("<display-name>", "Database"),
        ("<purpose>", "Persistence correctness"),
        ("<use-when>", "Persistence work"),
        ("<avoid-when>", "Prose only"),
        ("<positive-example>", "Repair rollback"),
        ("<negative-example>", "Edit docs"),
        ("<distinction>", "Writing handles prose"),
        ("<instructions>", "Check transaction boundaries"),
        ("<profile-revision>", "profile-r1"),
        ("<claim>", "Exact local model listing"),
        ("<finding>", "Owner connection reported"),
        ("<limitation>", "Execution is untested"),
        ("<reason>", "Owner local fixture selection"),
        ("<uncertainty>", "Comparative quality untested"),
    ];
    for (placeholder, value) in values {
        assert!(text.contains(placeholder), "{placeholder}");
        text = text.replace(placeholder, value);
    }
    assert!(!text.contains(['<', '>']), "unfilled placeholder: {text}");
    let file = fixture.root().join("filled.json");
    std::fs::write(&file, text).unwrap();

    // Then the real validate command accepts it.
    let facets = fixture.ok(&["validate", "--file", file.to_str().unwrap()]);
    assert_eq!(facets["configuration_valid"], true);
}

#[test]
fn docs_sample_is_the_shipped_template() {
    let docs = include_str!("../../../docs/profile-setup.md");
    let marker = "<!-- Byte-equal to `corral setup template`; a test enforces it. -->\n```json\n";
    let start = docs.find(marker).expect("template marker") + marker.len();
    let len = docs[start..].find("```").expect("closing fence");
    assert_eq!(&docs[start..start + len], corral_ops::SETUP_TEMPLATE);
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let key = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                files.insert(key, None);
                walk(root, &path, files);
            } else {
                files.insert(key, Some(std::fs::read(&path).unwrap()));
            }
        }
    }
    let root = root.join(".corral");
    let mut files = BTreeMap::new();
    walk(&root, &root, &mut files);
    files
}

#[test]
fn approved_setup_applies_saved_proposal_after_binary_restart() {
    // Given an exact, provisional proposal with independent fake harness observations.
    let fixture = Fixture::new();
    let (input, proposal_file) = fixture.proposed();
    let original = std::fs::read(fixture.config()).unwrap();
    let approval = fixture.write("approval.json", &fixture.approval(&input));
    let approved = fixture.ok(&[
        "approve",
        "--file",
        approval.to_str().unwrap(),
        "--expected-revision",
        "2",
        "--json",
    ]);
    assert_eq!(approved["revision"], 3);
    assert_eq!(std::fs::read(fixture.config()).unwrap(), original);
    assert_eq!(fixture.ok(&["status", "--json"])["record"], approved);
    fixture.ok(&["diff", "--json"]);
    std::fs::remove_file(proposal_file).unwrap();
    std::fs::remove_file(approval).unwrap();

    // When a new process applies with no caller input or discovery re-probe.
    let applied = fixture.ok(&["apply", "--json"]);

    // Then the saved approval and choices persist, without claiming execution verification.
    assert_eq!(applied["state"], "applied");
    assert_eq!(applied["revision"], 4);
    assert_eq!(applied["proposal"], approved["proposal"]);
    assert_eq!(applied["approval"], approved["approval"]);
    assert_eq!(applied["validation"]["configuration_valid"], true);
    assert_eq!(applied["validation"]["access_checked"], false);
    assert_eq!(applied["validation"]["execution_tested"], false);
    assert_eq!(fixture.ok(&["status"])["record"], applied);
    let catalog = fixture.ok(&["catalog", "--json"]);
    assert_eq!(
        catalog["profiles"]["custom.database"],
        approved["proposal"]["profiles"]["custom.database"]
    );
    assert_eq!(catalog["roles"]["planning"], "custom.database");
    assert!(!fixture.root().join(".corral/tasks").exists());
    assert!(!fixture.root().join(".corral/state/queue.json").exists());
}

#[test]
fn rejected_approval_preserves_config_and_journal_for_every_binding_failure() {
    // Given a proposed provisional setup and the exact approval it needs.
    let fixture = Fixture::new();
    let (input, _) = fixture.proposed();
    let baseline = snapshot(fixture.root());
    let approval = fixture.approval(&input);

    // When bindings, acknowledgement or optimistic revision are wrong.
    for field in [
        "proposal",
        "proposal_digest",
        "based_on_config_revision",
        "acknowledged_evidence_gap",
        "expected_revision",
    ] {
        let mut wrong = approval.clone();
        if field == "acknowledged_evidence_gap" {
            wrong[field] = json!(false);
        } else if field != "expected_revision" {
            wrong[field] = json!("not-the-approved-value");
        }
        let file = fixture.write("wrong-approval.json", &wrong);
        fixture.reject(&[
            "approve",
            "--file",
            file.to_str().unwrap(),
            "--expected-revision",
            if field == "expected_revision" {
                "1"
            } else {
                "2"
            },
        ]);

        // Then no rejected approval changes even a journal byte or creates a file.
        assert_eq!(snapshot(fixture.root()), baseline, "{field}");
    }
}

#[test]
fn invalid_discovery_or_proposal_never_changes_configuration_or_journal() {
    // Given a gathering setup and a well-formed candidate observation fixture.
    let fixture = Fixture::new();
    let input = fixture.input();
    fixture.ok(&["begin", "--expected-revision", "0", "--json"]);
    let baseline = snapshot(fixture.root());

    // When typed input or independent observations do not support the proposal.
    for case in ["unknown", "model", "effort", "connection", "access"] {
        let mut wrong = input.clone();
        match case {
            "unknown" => wrong["unexpected"] = json!(true),
            "model" => wrong["discovery"]["candidates"][0]["model"] = json!("other"),
            "effort" => {
                wrong["discovery"]["candidates"][0]["settings"][0]["effort"] = json!("low");
            }
            "connection" => {
                wrong["discovery"]["candidates"][0]["connection"] = json!("other");
            }
            "access" => {
                wrong["proposal"]["access"][0]["facets"]["access_verified"] = json!(true);
            }
            _ => unreachable!(),
        }
        let file = fixture.write("invalid.json", &wrong);
        fixture.reject(&[
            "propose",
            "--file",
            file.to_str().unwrap(),
            "--expected-revision",
            "1",
            "--json",
        ]);

        // Then neither the rejected proposal nor config observation is persisted.
        assert_eq!(snapshot(fixture.root()), baseline, "{case}");
    }
}

#[test]
fn read_only_commands_do_not_create_files_or_observe_raw_configuration_edits() {
    // Given no setup journal, event log or config observation cache.
    let fixture = Fixture::new();
    let input = fixture.write("proposal.json", &fixture.input());

    // When reading or validating, first pristine and then after a raw semantic config change.
    for changed in [false, true] {
        if changed {
            std::fs::write(
                fixture.config(),
                format!("{CONFIG}\n[profiles.manual]\nharness = \"fixture\"\n"),
            )
            .unwrap();
        }
        let baseline = snapshot(fixture.root());
        fixture.ok(&["status", "--json"]);
        fixture.ok(&["catalog"]);
        // No proposal is a valid diff error; it must still be structured and read-only.
        let diff = fixture.cli(&["diff"]);
        let bytes = if diff.status.success() {
            &diff.stdout
        } else {
            &diff.stderr
        };
        serde_json::from_slice::<Value>(bytes).unwrap();
        let args = ["validate", "--file", input.to_str().unwrap(), "--json"];
        if changed {
            fixture.reject(&args);
        } else {
            assert_eq!(fixture.ok(&args)["configuration_valid"], true);
        }

        // Then even the first read creates no directories, journal or config events.
        assert_eq!(snapshot(fixture.root()), baseline);
    }
}

#[test]
fn stale_config_blocks_apply_without_recording_the_raw_edit() {
    // Given an approved proposal, followed by an out-of-band configuration change.
    let fixture = Fixture::new();
    let (input, _) = fixture.proposed();
    let file = fixture.write("approval.json", &fixture.approval(&input));
    fixture.ok(&[
        "approve",
        "--file",
        file.to_str().unwrap(),
        "--expected-revision",
        "2",
    ]);
    // Apply takes the config lock before its stale check. Provision that empty
    // synchronization file so the snapshot measures persisted data, not lock creation.
    std::fs::write(fixture.root().join(".corral/lock/config.lock"), "").unwrap();
    std::fs::write(
        fixture.config(),
        format!("{CONFIG}\n[profiles.manual]\nharness = \"fixture\"\n"),
    )
    .unwrap();
    let baseline = snapshot(fixture.root());

    // When fresh processes inspect and try to apply the stale approved choices.
    fixture.ok(&["status"]);
    assert_eq!(fixture.ok(&["diff", "--json"])["stale"], true);
    assert_eq!(snapshot(fixture.root()), baseline);
    fixture.reject(&["apply"]);

    // Then the edit and proposed journal are unchanged and no observation event is appended.
    assert_eq!(snapshot(fixture.root()), baseline);
}

#[test]
fn cancel_persists_and_stale_mutations_are_read_only() {
    // Given a gathering setup.
    let fixture = Fixture::new();
    fixture.ok(&["begin", "--expected-revision", "0"]);
    let baseline = snapshot(fixture.root());
    fixture.reject(&["begin", "--expected-revision", "0"]);
    fixture.reject(&["cancel", "--expected-revision", "0"]);
    assert_eq!(snapshot(fixture.root()), baseline);

    // When cancellation is guarded by the current revision.
    let cancelled = fixture.ok(&["cancel", "--expected-revision", "1", "--json"]);

    // Then cancellation survives restart and leaves configured profiles alone.
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["revision"], 2);
    assert_eq!(fixture.ok(&["status"])["record"], cancelled);
    assert_eq!(std::fs::read_to_string(fixture.config()).unwrap(), CONFIG);
}

#[test]
fn malformed_or_unbounded_file_inputs_and_usage_errors_are_structured() {
    // Given a 16 KiB JSON bound and no setup record.
    let fixture = Fixture::new();
    let oversized = fixture.root().join("oversized.json");
    std::fs::write(&oversized, vec![b' '; 16385]).unwrap();
    let invalid = fixture.write("unknown.json", &json!({"unexpected":true}));
    let baseline = snapshot(fixture.root());

    // When callers submit non-files, malformed records, or omit required guards.
    for file in [
        fixture.root().to_str().unwrap(),
        oversized.to_str().unwrap(),
        invalid.to_str().unwrap(),
        "-",
    ] {
        for command in ["validate", "propose", "approve"] {
            let mut args = vec![command, "--file", file, "--json"];
            if command != "validate" {
                args.extend(["--expected-revision", "0"]);
            }
            fixture.reject(&args);
        }
    }
    for args in [
        vec!["begin"],
        vec!["propose", "--file", invalid.to_str().unwrap()],
        vec!["approve", "--file", invalid.to_str().unwrap()],
        vec!["cancel"],
        vec!["apply", "--file", invalid.to_str().unwrap()],
    ] {
        fixture.reject(&args);
    }

    // Then every failure is JSON stderr with nonzero status and no side effects.
    assert_eq!(snapshot(fixture.root()), baseline);
}

#[cfg(unix)]
#[test]
fn special_and_symlink_inputs_are_rejected_without_consuming_devices() {
    use std::os::unix::fs::symlink;

    // Given a symlink to otherwise valid JSON and unbounded special devices.
    let fixture = Fixture::new();
    let input = fixture.write("proposal.json", &fixture.input());
    let link = fixture.root().join("link.json");
    symlink(input, &link).unwrap();
    let baseline = snapshot(fixture.root());

    // When validation is asked to read those inputs.
    for file in ["/dev/zero", "/dev/null", link.to_str().unwrap()] {
        fixture.reject(&["validate", "--file", file]);
    }

    // Then device input is refused before reading and the workspace is untouched.
    assert_eq!(snapshot(fixture.root()), baseline);
}

#[cfg(unix)]
#[test]
fn discovery_only_runs_local_inventory_probes_and_never_caches_them() {
    use std::os::unix::fs::PermissionsExt;

    // Given a fake executable that accepts only inventory probes and a herdr tripwire.
    let fixture = Fixture::new();
    let harness = fixture.root().join("bin/fixture-harness");
    std::fs::write(
        &harness,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/probes\"\ncase \"$*\" in\n--version) printf 'fixture 1.0\\n';;\n--list-models) printf 'provider model\\nopenai exact-fixture-v1\\n';;\n*) exit 77;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
    let herdr = fixture.root().join("bin/herdr");
    std::fs::write(
        &herdr,
        "#!/bin/sh\nprintf contacted > herdr-contacted\nexit 77\n",
    )
    .unwrap();
    std::fs::set_permissions(herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let baseline = snapshot(fixture.root());

    // When the actual CLI discovers installed harnesses.
    let discovery = fixture.ok(&["discover", "--json"]);

    // Then inventory is sanitized and makes no access or execution claims.
    let observed = discovery["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|harness| harness["key"] == "fixture")
        .unwrap();
    assert_eq!(observed["installed"], true);
    assert_eq!(observed["models"], json!(["openai/exact-fixture-v1"]));
    assert_eq!(discovery["candidates"], json!([]));
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("home/probes")).unwrap(),
        "--version\n--list-models\n"
    );
    assert_eq!(snapshot(fixture.root()), baseline);
}
