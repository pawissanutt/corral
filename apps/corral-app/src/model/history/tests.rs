use super::graph::{Line, lanes};

/// A merge drawn with one edge lost its second parent, and a page's last commit, whose parent
/// is further down the walk, was drawn as where the history starts.
#[test]
fn a_merge_keeps_both_parent_edges_and_a_missing_parent_continues_below() {
    let parents = |list: &[&str]| list.iter().map(|sha| (*sha).to_owned()).collect::<Vec<_>>();
    let (merge, side, main) = (
        parents(&["main", "side"]),
        parents(&["main"]),
        parents(&["older"]),
    );
    let commits = [
        ("merge", merge.as_slice()),
        ("side", side.as_slice()),
        ("main", main.as_slice()),
    ];
    let graph = lanes(&commits);
    let merged = &graph.rows[0];
    assert_eq!(merged.parents.len(), 2);
    assert_ne!(merged.parents[0].lane, merged.parents[1].lane);
    assert!(merged.parents.iter().all(|edge| edge.loaded));
    for edge in &merged.parents {
        assert!(merged.lower.contains(&Line {
            from: merged.column,
            to: edge.lane
        }));
    }
    // The side commit joins the merge's first-parent lane, which `main` then keeps.
    let (side_row, main_row) = (&graph.rows[1], &graph.rows[2]);
    assert_eq!(side_row.column, merged.parents[1].lane);
    assert!(side_row.lower.contains(&Line {
        from: side_row.column,
        to: merged.parents[0].lane
    }));
    assert_eq!(main_row.column, merged.column);
    assert!(!main_row.root && !main_row.parents[0].loaded);
    assert_eq!(graph.continuing.len(), 1);
    assert_eq!(graph.continuing[0].parent, "older");
    assert!(graph.rows.iter().all(|row| !row.root));
    let root = lanes(&[("first", &[][..])]);
    assert!(root.rows[0].root && root.continuing.is_empty());
}

/// Only the newest eight branches were offered, with no way to read an older one.
#[test]
fn typing_reaches_a_branch_past_those_offered() {
    use super::{Action, State, page};
    use crate::model::daemon::Feed;
    use crate::model::projects::Scope;
    use corral_protocol::rows::ProjectRow;
    let names: Vec<String> = (0..20).map(|n| format!("topic/{n:02}")).collect();
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert(
        "corral".into(),
        ProjectRow {
            id: "corral".into(),
            local_branches: Some(names),
            ..ProjectRow::default()
        },
    );
    let scope = Scope::Project("corral".into());
    let offered = |state: &State| -> Vec<String> {
        page(state, &feed, &scope)
            .branches
            .into_iter()
            .map(|choice| choice.id)
            .collect()
    };
    let mut state = State::default();
    state.act(Action::Follow("corral".into()));
    let first = offered(&state);
    assert!(first.contains(&"seg-history-branch-topic/00".to_owned()));
    assert!(!first.contains(&"seg-history-branch-topic/19".to_owned()));
    state.act(Action::Type("19".into()));
    assert_eq!(
        offered(&state),
        ["seg-history-branch-HEAD", "seg-history-branch-topic/19"]
    );
    state.act(Action::Type("gone/branch".into()));
    state.act(Action::Submit);
    assert_eq!(state.branch.as_deref(), Some("gone/branch"));
    let shown = page(&state, &feed, &scope);
    assert_eq!(
        shown.target,
        Some(super::target("corral", Some("gone/branch")))
    );
    assert!(
        shown
            .branches
            .iter()
            .any(|choice| choice.chosen && choice.id == "seg-history-branch-gone/branch")
    );
    state.act(Action::Type(String::new()));
    state.act(Action::Submit);
    assert_eq!(state.branch, None);
}
