use corral_protocol::detail::DetailBody;
use corral_types::{
    ContainerFact, ContainerKind, DependentFact, SystemCategorySummary, SystemNodeStats,
    WorkspaceStatus,
};

use super::*;
use crate::model::daemon::{ClientError, DetailCache, detail_key};
use crate::model::system::marks::Mark;

fn container_fact(path: &str) -> ContainerFact {
    ContainerFact {
        path: path.into(),
        name: path.into(),
        kind: ContainerKind::Cargo,
        workspace_status: WorkspaceStatus::Member,
        other_packages: Vec::new(),
    }
}

fn container(path: &str) -> Row {
    Row::Map {
        row: SystemMapRow::Container {
            container: container_fact(path),
            stats: SystemNodeStats::default(),
        },
    }
}

fn review(categories: Vec<SystemCategorySummary>, rows: Vec<Row>) -> SystemReview {
    SystemReview {
        task: "T-1".into(),
        project: "p".into(),
        base: "base-commit".into(),
        base_estimated: false,
        head: "head-commit".into(),
        status: SystemStatus::Ready,
        error: None,
        provider: None,
        skipped_non_utf8_paths: 0,
        omitted_rows: 0,
        categories,
        rows,
    }
}

fn module(container: &str, path: &str) -> ModuleId {
    ModuleId {
        container: container.into(),
        path: path.into(),
    }
}

/// A category nothing supplies must not read as an empty answer, and a package both touched
/// and holding dependents keeps both marks, as do the module and nested package holding one.
#[test]
fn unsupplied_category_is_not_zero_and_dependents_survive_a_touched_package() {
    let categories = Category::ALL
        .into_iter()
        .map(|category| SystemCategorySummary {
            category,
            provided: category != Category::Definitions,
            total: if category == Category::Dependents {
                2
            } else {
                0
            },
        })
        .collect();
    let dependent = |path: &str, module: Option<ModuleId>| Row::Dependent {
        dependent: DependentFact {
            path: path.into(),
            name: "x".into(),
            line: 4,
            of: "y".into(),
        },
        module,
    };
    let rows = vec![
        container("a"),
        container("a/b"),
        container("c"),
        Row::Container {
            path: "a".into(),
            added: 3,
            removed: 1,
        },
        dependent("a/src/other.rs", Some(module("a", "other"))),
        dependent("a/b/src/x.rs", None),
    ];
    let impact = impact(&review(categories, rows), &BTreeSet::new());
    let supply = |category: Category| {
        let index = Category::ALL.iter().position(|c| *c == category);
        impact
            .sections
            .iter()
            .find(|s| Some(s.index) == index)
            .map(|s| s.supply)
    };
    assert_eq!(supply(Category::Definitions), Some(Supply::NotProvided));
    assert_eq!(supply(Category::CallEdges), Some(Supply::None));
    assert_eq!(supply(Category::Dependents), Some(Supply::Rows(2)));
    let mark = |node: Node| impact.marks.get(&node).copied();
    assert_eq!(
        mark(Node::Container("a".into())),
        Some(Mark {
            touched: true,
            added: 3,
            removed: 1,
            dependents: 1,
        })
    );
    assert_eq!(
        mark(Node::Module(module("a", "other"))).map(|m| (m.touched, m.dependents)),
        Some((false, 1))
    );
    assert_eq!(
        mark(Node::Container("a/b".into())).map(|m| (m.touched, m.dependents)),
        Some((false, 1))
    );
    assert_eq!(mark(Node::Container("c".into())), None);
}

/// corrald anchors a call group at the base when every call in it was removed; the anchor
/// must name the base, where that line exists.
#[test]
fn a_removal_only_call_group_is_anchored_at_the_base() {
    let call = |added, removed| Row::CallEdge {
        from: module("a", "x"),
        to: module("a", "y"),
        added,
        removed,
        changed: 0,
        path: "a/src/x.rs".into(),
        line: 9,
    };
    let impact = impact(
        &review(Vec::new(), vec![call(0, 2), call(1, 1)]),
        &BTreeSet::new(),
    );
    let calls = Category::ALL
        .iter()
        .position(|c| *c == Category::CallEdges)
        .and_then(|index| impact.sections.get(index))
        .map(|section| {
            section
                .items
                .iter()
                .map(|item| item.anchor.as_ref().map(|a| (a.base, a.commit.clone())))
                .collect::<Vec<_>>()
        });
    assert_eq!(
        calls,
        Some(vec![
            Some((true, "base-commit".into())),
            Some((false, "head-commit".into())),
        ])
    );
}

fn cached(target: DetailRef, body: Option<DetailBody>, stale: bool, failed: bool) -> DetailCache {
    DetailCache {
        transcript_seams: Default::default(),
        pages: Vec::new(),
        loading: false,
        stale,
        error: failed.then(|| ClientError {
            code: "unavailable".into(),
            message: "corrald is not reachable".into(),
        }),
        merged: body,
        target,
    }
}

/// A body kept after its revalidation failed is retained, map included, but never shown as
/// confirmed.
#[test]
fn a_retained_impact_after_a_failed_read_is_marked_stale() {
    let target = request("T-1", None);
    let body = DetailBody::SystemReview(review(Vec::new(), vec![container("a")]));
    for (stale, failed, marked) in [
        (false, false, false),
        (true, false, true),
        (false, true, true),
    ] {
        let mut feed = Feed::default();
        feed.details.insert(
            detail_key(&target),
            cached(target.clone(), Some(body.clone()), stale, failed),
        );
        let Shown::Ready(impact) = shown(&feed, "T-1", &[], &BTreeSet::new()) else {
            panic!("the retained impact is not shown");
        };
        assert!(impact.scene.is_some());
        assert_eq!(impact.notices.iter().any(|n| n == STALE), marked);
    }
}

/// Activity › System draws the current head: a package only the submission adds is not
/// there, so Open in System must not promise it; one it has is opened with it selected.
#[test]
fn open_in_system_offers_only_what_the_system_map_has() {
    let system = request_map("p");
    let mut feed = Feed::default();
    let map = SystemMap {
        project: "p".into(),
        head: Some("main".into()),
        status: SystemStatus::Ready,
        error: None,
        provider: None,
        coverage: None,
        omitted_rows: 0,
        rows: vec![SystemMapRow::Container {
            container: container_fact("old"),
            stats: SystemNodeStats::default(),
        }],
    };
    feed.details.insert(
        detail_key(&system),
        cached(system, Some(DetailBody::SystemMap(map)), false, false),
    );
    let target = |path: &str| Target {
        trail: Vec::new(),
        node: Node::Container(path.into()),
    };
    assert!(matches!(
        reach(&feed, "p", &target("new")),
        Reach::Unavailable(_)
    ));
    assert!(matches!(
        reach(&feed, "p", &target("old")),
        Reach::Open { node: Node::Container(path), note: None, .. } if path == "old"
    ));
}

fn request_map(project: &str) -> DetailRef {
    crate::model::system::request(project, None)
}

/// A file the change deleted exists at the base only: its item is anchored there, and the map
/// selects the package at the head that held it, never the absent file.
#[test]
fn a_deleted_file_is_anchored_at_the_base_and_selects_its_surviving_package() {
    let rows = vec![
        container("a"),
        Row::File {
            path: "a/tests/gone.rs".into(),
            container: "a".into(),
            module: module("a", "tests"),
            added: 0,
            removed: 7,
            binary: false,
            deleted: true,
        },
        Row::TestFile {
            path: "a/tests/gone.rs".into(),
        },
    ];
    let impact = impact(&review(Vec::new(), rows), &BTreeSet::new());
    let item = impact
        .sections
        .iter()
        .flat_map(|s| s.items.iter())
        .find(|item| item.main == "a/tests/gone.rs");
    assert_eq!(
        item.map(|i| (
            i.anchor.as_ref().map(|a| (a.base, a.commit.clone())),
            i.target.as_ref().map(|t| t.node.clone()),
            i.note.is_some()
        )),
        Some((
            Some((true, "base-commit".into())),
            Some(Node::Container("a".into())),
            true
        ))
    );
}

/// Open in System promises a node only from confirmed levels read at one head: a level being
/// revalidated, or a package level from another head than the project level, is still checked.
#[test]
fn open_in_system_waits_for_confirmed_levels_at_one_head() {
    let map = |head: &str, rows: Vec<SystemMapRow>| SystemMap {
        project: "p".into(),
        head: Some(head.into()),
        status: SystemStatus::Ready,
        error: None,
        provider: None,
        coverage: None,
        omitted_rows: 0,
        rows,
    };
    let project = map(
        "new",
        vec![SystemMapRow::Container {
            container: container_fact("a"),
            stats: SystemNodeStats::default(),
        }],
    );
    let package = |head: &str| {
        map(
            head,
            vec![SystemMapRow::Module {
                module: module("a", "x"),
                stats: SystemNodeStats::default(),
            }],
        )
    };
    let at = SystemAt::Container { path: "a".into() };
    let target = Target {
        trail: vec![at.clone()],
        node: Node::Module(module("a", "x")),
    };
    let feed_with = |package: SystemMap, stale: bool| {
        let mut feed = Feed::default();
        for (target, body, stale) in [
            (request_map("p"), project.clone(), false),
            (crate::model::system::request("p", Some(&at)), package, stale),
        ] {
            feed.details.insert(
                detail_key(&target),
                cached(target, Some(DetailBody::SystemMap(body)), stale, false),
            );
        }
        feed
    };
    let checking = |feed: &Feed| matches!(reach(feed, "p", &target), Reach::Checking);
    assert!(checking(&feed_with(package("new"), true)));
    assert!(checking(&feed_with(package("old"), false)));
    assert!(matches!(
        reach(&feed_with(package("new"), false), "p", &target),
        Reach::Open { note: None, .. }
    ));
}
