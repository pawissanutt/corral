use corral_types::{
    ContainerFact, ContainerKind, DependentFact, SystemCategorySummary, SystemNodeStats,
    WorkspaceStatus,
};

use super::*;

fn container(path: &str) -> Row {
    Row::Map {
        row: SystemMapRow::Container {
            container: ContainerFact {
                path: path.into(),
                name: path.into(),
                kind: ContainerKind::Cargo,
                workspace_status: WorkspaceStatus::Member,
                other_packages: Vec::new(),
            },
            stats: SystemNodeStats::default(),
        },
    }
}

fn review(categories: Vec<SystemCategorySummary>, rows: Vec<Row>) -> SystemReview {
    SystemReview {
        task: "T-1".into(),
        project: "p".into(),
        base: "b".into(),
        base_estimated: false,
        head: "h".into(),
        status: SystemStatus::Ready,
        error: None,
        provider: None,
        skipped_non_utf8_paths: 0,
        omitted_rows: 0,
        categories,
        rows,
    }
}

/// A category nothing supplies must not read as an empty answer, and a dependent package must
/// be marked apart from a touched one, even in the touched package's nested container.
#[test]
fn unsupplied_category_is_not_zero_and_dependents_are_marked_apart() {
    let categories = Category::ALL
        .into_iter()
        .map(|category| SystemCategorySummary {
            category,
            provided: category != Category::Definitions,
            total: usize::from(category == Category::Dependents),
        })
        .collect();
    let rows = vec![
        container("a"),
        container("a/b"),
        container("c"),
        Row::Container {
            path: "a".into(),
            added: 3,
            removed: 1,
        },
        Row::Dependent {
            dependent: DependentFact {
                path: "a/b/src/x.rs".into(),
                name: "x".into(),
                line: 4,
                of: "y".into(),
            },
        },
    ];
    let impact = impact(&review(categories, rows), &BTreeSet::new());
    let figure = |title: &str| {
        impact
            .sections
            .iter()
            .find(|s| s.title == title)
            .map(|s| s.figure.clone())
    };
    assert_eq!(figure("Definitions").as_deref(), Some("not provided"));
    assert_eq!(figure("Call edges").as_deref(), Some("none"));
    assert_eq!(figure("Dependents").as_deref(), Some("1 item"));
    assert_eq!(
        impact.marks.get(&Node::Container("a".into())),
        Some(&Mark::Touched {
            added: 3,
            removed: 1
        })
    );
    assert_eq!(
        impact.marks.get(&Node::Container("a/b".into())),
        Some(&Mark::Dependent { definitions: 1 })
    );
    assert_eq!(impact.marks.get(&Node::Container("c".into())), None);
}
