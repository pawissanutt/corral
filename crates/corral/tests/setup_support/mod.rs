//! Private setup workspace shared by setup CLI test binaries.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::ProfileSetupProposal;
use serde_json::{Value, json};

pub const NOW: &str = "2026-10-18T12:00:00Z";
pub const CONFIG: &str = r#"
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

pub struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
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

    pub fn root(&self) -> &Path {
        self.temp.path()
    }

    pub fn config(&self) -> PathBuf {
        self.root().join(".corral/corral.toml")
    }

    pub fn cli(&self, args: &[&str]) -> Output {
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

    pub fn ok(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{args:?}: {:?}", output.stderr);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub fn reject(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {:?}", output.stdout);
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"].is_string(), "{error}");
        error
    }

    pub fn write(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.root().join(name);
        std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
    }

    pub fn input(&self) -> Value {
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

    pub fn proposed(&self) -> (Value, PathBuf) {
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

    pub fn approval(&self, input: &Value) -> Value {
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

pub fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
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
