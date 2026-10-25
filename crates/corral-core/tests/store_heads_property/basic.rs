//! The property suite's original eleven steps: head field and child mutations and full
//! old-row edits, each mirrored on the expected task.

use super::*;

pub(super) fn basic_step(
    store: &Store,
    expected: &mut Task,
    rng: &mut Rng,
    n: usize,
    choice: usize,
) -> String {
    let run = current_id(expected);
    match choice {
        0 => {
            let title = format!("Title {n}");
            store
                .update_head("T-001", |h| {
                    h.title = title.clone();
                    Ok(())
                })
                .unwrap();
            expected.title = title;
            "head title".into()
        }
        1 => {
            let summary = Some(format!("current {n}"));
            store
                .update_head("T-001", |h| {
                    h.current_run_mut().unwrap().summary = summary.clone();
                    Ok(())
                })
                .unwrap();
            expected.runs.last_mut().unwrap().summary = summary;
            "current run field".into()
        }
        2 => {
            let comment = TaskComment {
                author: CommentAuthor::Owner,
                at: at(n as i64),
                text: format!("comment {n}"),
            };
            store
                .update_head("T-001", |h| h.add_comment(comment.clone()))
                .unwrap();
            expected.comments.push(comment);
            "add_comment".into()
        }
        3 => {
            let k = expected.runs.last().unwrap().review_runs.len() + 1;
            let child: Run =
                serde_json::from_value(review_child("T-001", &run, k, 1, true, "approve")).unwrap();
            store
                .update_head("T-001", |h| h.push_review_child(&run, child.clone()))
                .unwrap();
            expected.runs.last_mut().unwrap().review_runs.push(child);
            "push_review_child".into()
        }
        4 => {
            let wait = wait_record(&run, n);
            store
                .update_head("T-001", |h| h.push_wait(&run, wait.clone()))
                .unwrap();
            expected
                .runs
                .last_mut()
                .unwrap()
                .obligations
                .waits
                .push(wait);
            "push_wait".into()
        }
        5 => {
            let switch: ProfileSwitch = serde_json::from_value(switch(n)).unwrap();
            store
                .update_head("T-001", |h| h.push_switch(&run, switch.clone()))
                .unwrap();
            expected.runs.last_mut().unwrap().switches.push(switch);
            "push_switch".into()
        }
        6 => {
            store
                .update_head("T-001", |h| {
                    h.append_usage_entries(&run, None, vec![entry(n)])
                })
                .unwrap();
            let current = expected.runs.last_mut().unwrap();
            let mut all = current.usage.take().map(|u| u.entries).unwrap_or_default();
            all.push(entry(n));
            current.usage = Some(Usage::from_entries(all));
            "append_usage_entries".into()
        }
        7 => {
            let children = &expected.runs.last().unwrap().review_runs;
            let child = children[rng.below(children.len())].id.clone();
            let summary = Some(format!("child {n}"));
            store
                .update_head("T-001", |h| {
                    h.edit_review_child(&run, &child, |c| {
                        c.summary = summary.clone();
                        Ok(())
                    })
                })
                .unwrap();
            let current = expected.runs.last_mut().unwrap();
            current
                .review_runs
                .iter_mut()
                .find(|c| c.id == child)
                .unwrap()
                .summary = summary;
            "edit_review_child".into()
        }
        8 => {
            let old = rng.below(expected.runs.len() - 1);
            let summary = Some(format!("old run {n}"));
            store
                .update("T-001", |t| {
                    t.runs[old].summary = summary.clone();
                    Ok(())
                })
                .unwrap();
            expected.runs[old].summary = summary;
            "full old-run edit".into()
        }
        9 => {
            let old = rng.below(expected.runs.len() - 1);
            let k = rng.below(expected.runs[old].review_runs.len());
            let summary = Some(format!("old child {n}"));
            store
                .update("T-001", |t| {
                    t.runs[old].review_runs[k].summary = summary.clone();
                    Ok(())
                })
                .unwrap();
            expected.runs[old].review_runs[k].summary = summary;
            "full old-child edit".into()
        }
        _ => {
            let child = expected.runs.last().unwrap().review_runs[0].id.clone();
            let seq = store.view().unwrap().seq();
            let refused = store.update_head("T-001", |h| {
                h.edit_review_child(&run, &child, |c| {
                    c.id = "renamed".into();
                    Ok(())
                })
            });
            assert!(refused.is_err(), "an id change is refused");
            assert_eq!(
                store.view().unwrap().seq(),
                seq,
                "a refused edit writes nothing"
            );
            "refused id edit".into()
        }
    }
}
