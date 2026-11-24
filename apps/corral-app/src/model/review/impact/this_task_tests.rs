use corral_types::{ChangeKind, DependentFact, SystemAt};

use super::tests::{module, review};
use super::this_task::of_review;
use super::*;

fn file(path: &str, added: u64, removed: u64, deleted: bool) -> Row {
    Row::File {
        path: path.into(),
        container: "a".into(),
        module: module("a", "m"),
        added,
        removed,
        binary: false,
        deleted,
    }
}

/// A package's churn counted a deleted file's lines while its file count left the file out,
/// so an edit and a deletion read "1 file" beside the churn of two.
#[test]
fn a_package_counts_its_deleted_files_beside_their_churn() {
    let rows = vec![
        Row::Container {
            path: "a".into(),
            added: 3,
            removed: 10,
        },
        Row::Module {
            module: module("a", "m"),
            added: 3,
            removed: 10,
        },
        file("a/m/kept.rs", 3, 1, false),
        file("a/m/gone.rs", 0, 9, true),
        Row::Container {
            path: "b".into(),
            added: 0,
            removed: 4,
        },
        Row::File {
            path: "b/only.rs".into(),
            container: "b".into(),
            module: module("b", "m"),
            added: 0,
            removed: 4,
            binary: false,
            deleted: true,
        },
    ];
    let marks = impact(&review(Vec::new(), rows), None).marks;
    let a = marks[&Node::Container("a".into())];
    assert_eq!((a.changed, a.files), (2, 1));
    assert_eq!(marks[&Node::Module(module("a", "m"))].changed, 2);
    let b = marks[&Node::Container("b".into())];
    assert_eq!((b.changed, b.files), (1, 0));
}

/// Selecting a "N more" card or a definition explained nothing of the task under it.
#[test]
fn a_folded_card_and_a_definition_say_what_the_task_did_there() {
    let rows = vec![
        file("a/m/kept.rs", 3, 1, false),
        Row::Definition {
            change: ChangeKind::Changed,
            path: "a/m/kept.rs".into(),
            name: "f".into(),
            line: 2,
        },
        Row::Dependent {
            dependent: DependentFact {
                path: "a/m/user.rs".into(),
                name: "g".into(),
                line: 5,
                of: "kept::f".into(),
                depth: None,
            },
            module: Some(module("a", "m")),
        },
    ];
    let review = review(Vec::new(), rows);
    let at = SystemAt::Module {
        module: module("a", "m"),
    };
    // A level that lists no file card folds every file into its "N more" card.
    let scene = build(&head_map(&review), Some(&at));
    let folded = of_review(&review, &Node::More, &scene, Some(&at));
    assert_eq!(
        folded.map(|t| (t.files.len(), t.dependents.len())),
        Some((1, 1))
    );
    let at = SystemAt::File {
        path: "a/m/kept.rs".into(),
    };
    let definition = Node::Definition {
        line: 2,
        name: "f".into(),
    };
    let changed = of_review(&review, &definition, &scene, Some(&at));
    assert_eq!(changed.map(|t| t.files.len()), Some(1));
}
