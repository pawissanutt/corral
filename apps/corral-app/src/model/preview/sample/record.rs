//! The sample tasks' planning metadata and seeded records.

use corral_protocol::detail::TaskDetail;
use corral_types::{Task, TaskDifficulty};

use super::{Seed, ago, papers, work};
use crate::model::daemon::Feed;

const PLAN_BODY: &str = concat!(
    "## Goal\n",
    "The app runs on the simple engine: one task list, one Actions menu, typed refusals.\n",
    "\n",
    "## Steps\n",
    "1. Shared Actions menu (T-2)\n",
    "2. Restore paused sessions (T-8)\n",
    "3. Narrow sidebar (T-11)\n",
    "4. Real timeline dates (T-14), then zoom presets (T-12)\n",
    "\n",
    "## Acceptance\n",
    "- Every engine operation is on every task row, enabled; the engine decides.\n",
    "- A refusal shows its reason and one button per next step, the forced one last.\n",
    "- A destructive or forced operation asks once, on this device.\n",
    "- Review lands onto the run's base unless another existing branch is named.\n",
    "\n",
    "## Order\n",
    "T-12 waits on T-14: zoom presets need the real dates first. The rest can run side by\n",
    "side; each lands on its own once its gate and review pass.\n",
    "\n",
    "## Notes\n",
    "Builds merge into Agents; the Builds destination was dropped (T-3).\n",
    "The plan is done when every member is done and the owner marks T-1 done.",
);

/// The size, difficulty and tags of the sample tasks that have them; T-8 keeps a size from
/// before the scale, and T-11 a custom tag with a blank.
fn planning(
    id: &str,
) -> (
    Option<&'static str>,
    Option<TaskDifficulty>,
    &'static [&'static str],
) {
    use TaskDifficulty::{Hard, Routine, Standard};
    match id {
        "T-8" => (Some("medium"), None, &[]),
        "T-9" => (Some("s"), Some(Hard), &["bug"]),
        "T-10" => (Some("m"), Some(Standard), &["documentation", "testing"]),
        "T-11" => (Some("xs"), Some(Routine), &["accessibility", "ui polish"]),
        "T-12" => (Some("s"), Some(Routine), &["performance"]),
        "T-13" => (Some("m"), Some(Hard), &["testing"]),
        _ => (None, None, &[]),
    }
}

pub(super) fn record(feed: &Feed, seed: &Seed) -> TaskDetail {
    let (size, difficulty, tags) = planning(seed.id);
    let body = if seed.id == "T-1" {
        PLAN_BODY.to_owned()
    } else {
        format!(
            "## Goal\n{}\n\n## Acceptance\n- The behaviour is covered by one test at the lowest layer.",
            seed.goal
        )
    };
    let mut record = TaskDetail {
        pending_review_verdict: None,
        task: Task {
            id: seed.id.into(),
            title: seed.title.into(),
            body,
            status: seed.status,
            project: seed.project.map(str::to_owned),
            priority: seed.priority,
            kind: (seed.id == "T-1").then(|| "plan".into()),
            size: size.map(str::to_owned),
            difficulty,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            profile: None,
            base: None,
            parent: seed.parent.map(str::to_owned),
            depends_on: Vec::new(),
            blocked: None,
            attention: None,
            round: 0,
            created: ago(feed, seed.created),
            updated: ago(feed, seed.updated),
        },
        runs: Vec::new(),
        checks: Vec::new(),
        artifacts: Vec::new(),
        events: Vec::new(),
    };
    work(feed, &mut record);
    papers(feed, &mut record);
    record
}
