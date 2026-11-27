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
