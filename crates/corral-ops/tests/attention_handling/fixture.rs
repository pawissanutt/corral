use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use corral_core::{Run, Status, Store, Task};
use corral_herdr::Herdr;
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::Ctx;

pub fn at(seconds: i64) -> Timestamp {
    "2099-01-01T00:00:00Z"
        .parse::<Timestamp>()
        .unwrap()
        .checked_add(SignedDuration::from_secs(seconds))
        .unwrap()
}

pub struct Fx {
    pub tmp: tempfile::TempDir,
    pub ctx: Ctx,
    pub bin: PathBuf,
}

impl Fx {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("herdr");
        std::fs::write(&bin, r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") test ! -f "$dir/offline" || exit 1; printf '{"result":{"workspaces":[]}}\n' ;;
  "pane list") test ! -f "$dir/inventory-down" || exit 1
    printf '{"result":{"panes":[{"pane_id":"worker:p1","workspace_id":"worker","agent_status":"working"}]}}\n' ;;
  "pane get") printf '{"result":{"pane":{"pane_id":"%s","workspace_id":"worker","agent_status":"working"}}}\n' "$3" ;;
  "pane read") printf 'Working\n' ;;
  "notification show") printf 'called\n' >> "$dir/notifications"
    test ! -f "$dir/notify-fail" || exit 1; printf '{"result":{}}\n' ;;
  *) printf '{"result":{}}\n' ;;
esac
"#).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Keep fresh-store reconciliation real without reparsing unrelated starter catalogs.
        let store = Store::init_with(tmp.path(), |store| {
            std::fs::write(
                store.config_path(),
                "[agents.omo]\ncommand = \"true\"\n[pm]\nagent = \"omo\"\n",
            )?;
            Ok(())
        })
        .unwrap();
        Self {
            ctx: Ctx {
                store,
                herdr: Herdr::with_bin(bin.clone(), None),
                resource_probe: crate::resources::FixedResourceProbe::arc(),
            },
            tmp,
            bin,
        }
    }

    pub fn reopen(&self) -> Ctx {
        Ctx {
            store: Store::open(self.ctx.store.root()).unwrap(),
            herdr: Herdr::with_bin(self.bin.clone(), None),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        }
    }

    pub fn stalled(&self, project: Option<&str>) -> Task {
        self.ctx
            .store
            .create_initialized("Stalled review", project, Status::Review, "", |task| {
                task.runs.push(serde_json::from_value(json!({
                    "id":"R1","agent":"fixture","worktree":"/missing-fixture-worktree",
                    "branch":"corral/T","base":"main","base_sha":"abc", "started":at(0),
                    "outcome":"done", "autopilot":{"stage":"reviewing","at":at(0),"sha":"abc"}
                }))?);
                Ok(())
            })
            .unwrap()
    }

    pub fn questions(&self, count: usize) -> Task {
        self.ctx.store.create_initialized("Questions", None, Status::Running, "", |task| {
            let mut run: Run = serde_json::from_value(json!({
                "id":"R1","agent":"fixture","worktree":"/missing-fixture-worktree",
                "branch":"corral/T","base":"main","base_sha":"abc","started":at(0),
                "agent_state":"working","delivery":"inbox"
            }))?;
            let mut state = AgentRunState::new(AgentBinding {
                role: AgentRole::Worker, store: "store".into(), project: "app".into(),
                generation: 1, health: HostHealth::Healthy, callback: None, closing: false,
            });
            for index in 1..=count {
                let key = format!("{}/R1/Q{index}", task.id);
                state.questions.push(serde_json::from_value(json!({
                    "version":1,"key":key,"revision":1,"kind":"clarification","blocking":false,
                    "text":"Which constraint applies?","state":"open",
                    "asked":{"request_id":key,"payload_sha256":"sha","session_id":"session","generation":1,"at":at(0)},
                    "wake":{"route":"none","key":key,"state":"pending"}
                }))?);
            }
            run.agent_host = Some(Box::new(state));
            task.runs.push(run);
            Ok(())
        }).unwrap()
    }

    pub fn calls(&self) -> usize {
        std::fs::read_to_string(self.tmp.path().join("notifications"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    pub fn logical(&self, kind: &str) -> BTreeSet<String> {
        self.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|event| event.kind == kind)
            .map(|event| event.detail["fact_id"].as_str().unwrap().into())
            .collect()
    }
}
