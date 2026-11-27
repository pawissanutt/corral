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

pub(super) fn review(categories: Vec<SystemCategorySummary>, rows: Vec<Row>) -> SystemReview {
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

pub(super) fn module(container: &str, path: &str) -> ModuleId {
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
            depth: None,
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
    let impact = impact(&review(categories, rows), None);
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
            files: 0,
            file_added: 0,
            file_removed: 0,
            changed: 0,
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

/// Runtime and entry point rows were dropped and both categories hidden, which also shifted
/// the later sections' places: each lists its rows under its own place in `Category::ALL`, and
/// an entry point the change removed is anchored at the base.
#[test]
fn runtime_and_entry_point_changes_are_listed_in_their_categories() {
    use corral_types::{
        EntryKind, EntryPoint, RuntimeSource, RuntimeUnit, RuntimeUnitId, RuntimeUnitKind,
    };
    let unit = |ports: &[&str]| RuntimeUnit {
        id: RuntimeUnitId {
            kind: RuntimeUnitKind::Service,
            name: "api".into(),
            path: "compose.yaml".into(),
        },
        line: 12,
        container: String::new(),
        command: None,
        entry_point: None,
        image: None,
        build_context: None,
        dockerfile: None,
        ports: ports.iter().map(|p| (*p).to_owned()).collect(),
        depends_on: Vec::new(),
    };
    let categories = Category::ALL
        .into_iter()
        .map(|category| SystemCategorySummary {
            category,
            provided: category != Category::Tests,
            total: usize::from(matches!(
                category,
                Category::Runtime | Category::EntryPoints
            )),
        })
        .collect();
    let rows = vec![
        container("a"),
        Row::RuntimeUnit {
            before: Some(unit(&["80"])),
            after: Some(unit(&["8080"])),
        },
        Row::Entry {
            change: ChangeKind::Removed,
            entry: EntryPoint {
                kind: EntryKind::Route,
                name: "GET /tasks".into(),
                path: "a/src/routes.rs".into(),
                line: 7,
                owner: RuntimeSource::Container { path: "a".into() },
            },
        },
    ];
    let impact = impact(&review(categories, rows), None);
    let section = |category: Category| {
        let index = Category::ALL.iter().position(|c| *c == category);
        impact.sections.iter().find(|s| Some(s.index) == index)
    };
    let runtime = section(Category::Runtime).map(|s| (s.supply, s.groups[0].items.clone()));
    let Some((Supply::Rows(1), items)) = runtime else {
        panic!("runtime section: {runtime:?}");
    };
    assert_eq!(
        (items[0].sign, items[0].right.as_str()),
        (Sign::Changed, "ports 80 → 8080")
    );
    let entries = section(Category::EntryPoints).map(|s| s.groups[0].items.clone());
    let anchor = entries
        .as_ref()
        .and_then(|items| items.first())
        .and_then(|item| item.anchor.clone());
    assert_eq!(anchor.map(|a| (a.base, a.line)), Some((true, Some(7))));
    assert_eq!(
        section(Category::Tests).map(|s| s.supply),
        Some(Supply::NotProvided)
    );
    assert_eq!(impact.sections.len(), Category::ALL.len());
}

/// Before a configured extension's answer reaches the review, its categories wait for it
/// rather than reading "not provided"; once it answers, a listed category with no rows is none,
/// and an empty candidate answer still shows beside a built-in changed test file.
#[test]
fn an_extension_category_waits_for_its_answer_then_reads_none() {
    let categories = |provided| {
        Category::ALL
            .into_iter()
            .map(|category| SystemCategorySummary {
                category,
                provided: provided
                    || !items::provider_supplied(category)
                    || category == Category::Tests,
                total: usize::from(provided && category == Category::Tests),
            })
            .collect()
    };
    let supply = |impact: &Impact, category: Category| {
        let index = Category::ALL.iter().position(|c| *c == category);
        impact
            .sections
            .iter()
            .find(|s| Some(s.index) == index)
            .map(|s| s.supply)
    };
    let waiting = impact(&review(categories(false), Vec::new()), Some("ext"));
    assert_eq!(
        supply(&waiting, Category::Dependents),
        Some(Supply::Unanswered)
    );
    let mut answered = review(
        categories(true),
        vec![Row::TestFile {
            path: "a/tests/x.rs".into(),
        }],
    );
    answered.provider = Some(corral_types::SystemProvider {
        extension: "ext".into(),
        coverage: corral_types::ProviderCoverage {
            provided: vec!["changes".into(), "dependents".into(), "tests".into()],
            not_provided: Vec::new(),
            skipped: BTreeMap::new(),
            omitted: BTreeMap::new(),
            dropped_unknown_files: 0,
        },
        error: None,
    });
    let answered = impact(&answered, Some("ext"));
    assert_eq!(supply(&answered, Category::Definitions), Some(Supply::None));
    assert_eq!(supply(&answered, Category::Dependents), Some(Supply::None));
    let tests = Category::ALL
        .iter()
        .position(|c| *c == Category::Tests)
        .and_then(|index| answered.sections.get(index));
    assert_eq!(tests.map(|s| s.supply), Some(Supply::Rows(1)));
    let empty_answer = |section: &Section| {
        section
            .notes
            .iter()
            .any(|note| note.starts_with("No test candidate:"))
    };
    assert!(tests.is_some_and(empty_answer));
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
    let impact = impact(&review(Vec::new(), vec![call(0, 2), call(1, 1)]), None);
    let calls = Category::ALL
        .iter()
        .position(|c| *c == Category::CallEdges)
        .and_then(|index| impact.sections.get(index))
        .map(|section| {
            section
                .groups
                .iter()
                .flat_map(|g| &g.items)
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
        // The client keeps a failed read stale with its error.
        stale: stale || failed,
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
        let Shown::Ready(impact) = shown(&feed, "T-1", &[]) else {
            panic!("the retained impact is not shown");
        };
        assert!(impact.scene.is_some());
        assert_eq!(impact.notices.iter().any(|n| n == STALE), marked);
    }
}

/// Insight › Structure draws the current head: a package only the submission adds is not
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
    let impact = impact(&review(Vec::new(), rows), None);
    let item = impact
        .sections
        .iter()
        .flat_map(|s| &s.groups)
        .flat_map(|g| &g.items)
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
            (
                crate::model::system::request("p", Some(&at)),
                package,
                stale,
            ),
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
    // A package level whose read failed is said, not checked for ever.
    let mut failed = feed_with(package("new"), false);
    let level = crate::model::system::request("p", Some(&at));
    failed.details.insert(
        detail_key(&level),
        cached(
            level.clone(),
            Some(DetailBody::SystemMap(package("new"))),
            false,
            true,
        ),
    );
    assert!(matches!(
        reach(&failed, "p", &target),
        Reach::Unavailable(_)
    ));
    // One System cannot find at its head opens the package that holds it, said so.
    let mut gone = cached(level.clone(), None, false, true);
    if let Some(error) = gone.error.as_mut() {
        error.code = "not_found".into();
    }
    failed.details.insert(detail_key(&level), gone);
    assert!(matches!(
        reach(&failed, "p", &target),
        Reach::Open { node: Node::Container(path), note: Some(_), .. } if path == "a"
    ));
    assert!(checking(&feed_with(package("old"), false)));
    assert!(matches!(
        reach(&feed_with(package("new"), false), "p", &target),
        Reach::Open { note: None, .. }
    ));
}

/// A data model change opens its own source line on the side that holds it: a removed field at
/// the base, a retyped one at the head, each with only the excerpt served for that line (one
/// served for another line would show the wrong source).
#[test]
fn data_changes_anchor_removals_at_the_base_with_their_own_excerpt() {
    use corral_types::{
        DataChange, DataChangeKind, DataField, DataKind, DataSource, SourceExcerpt,
    };
    let field = |name: &str, ty: &str, line| DataField {
        name: name.into(),
        ty: ty.into(),
        line,
        optional: false,
    };
    let change = |change| DataChange {
        path: "crates/t/src/lib.rs".into(),
        name: "Task".into(),
        line: 3,
        kind: DataKind::Record,
        source: DataSource::RustSerde,
        contract: true,
        change,
    };
    let excerpt = |commit: &str, line| SourceExcerpt {
        path: "crates/t/src/lib.rs".into(),
        commit: commit.into(),
        line,
        first: line - 1,
        lines: vec!["a".into(), "b".into(), "c".into()],
    };
    let rows = vec![
        container("crates/t"),
        Row::DataChange {
            change: change(DataChangeKind::FieldRemoved {
                field: field("pane", "String", 9),
            }),
            excerpt: Some(excerpt("base-commit", 9)),
        },
        Row::DataChange {
            change: change(DataChangeKind::FieldRetyped {
                before: field("round", "u32", 5),
                after: field("round", "u64", 6),
            }),
            excerpt: Some(excerpt("head-commit", 5)),
        },
    ];
    let categories = vec![SystemCategorySummary {
        category: Category::DataModel,
        provided: true,
        total: 2,
    }];
    let shown = impact(&review(categories, rows), None);
    let section = shown
        .sections
        .iter()
        .find(|s| Category::ALL.get(s.index) == Some(&Category::DataModel))
        .unwrap_or_else(|| panic!("no Data model section"));
    let [removed, retyped] = section.groups[0].items.as_slice() else {
        panic!("{:?}", section.groups);
    };
    let anchor = removed
        .anchor
        .as_ref()
        .map(|a| (a.line, a.base, a.commit.as_str()));
    assert_eq!(anchor, Some((Some(9), true, "base-commit")));
    let lines = removed.source.clone().flatten().map(|s| s.lines.len());
    assert_eq!(lines, Some(3));
    assert_eq!(retyped.right, "u32 → u64");
    let anchor = retyped.anchor.as_ref().map(|a| (a.line, a.base));
    assert_eq!(anchor, Some((Some(6), false)));
    assert_eq!(
        retyped.source,
        Some(None),
        "an excerpt for line 5 is not line 6's"
    );
}

fn reference(from: &ModuleId, to: &ModuleId, count: u64) -> SystemMapRow {
    SystemMapRow::ModuleReference {
        reference: corral_types::facts::Reference {
            from: from.clone(),
            to: to.clone(),
            kind: corral_types::ReferenceKind::Use,
            basis: corral_types::ReferenceBasis::Syntax,
            count,
        },
    }
}

/// The map drew no change on any edge and lost the change one level down: a moved reference
/// count reaches its edge at the project level (pairs folded) and at the package level, an
/// unchanged edge carries none, and a module level's "N more" card carries the changed file it
/// hides.
#[test]
fn moved_reference_counts_reach_their_edges_and_hidden_files_their_more_card() {
    use crate::model::system::Pick;
    use crate::model::system::marks::{drawn_pick, figure};
    use crate::model::system::scene::MAX_FILES;
    use corral_types::{PackageEdge, PackageEdgeKind, ReadStatus};
    let (x, z, y) = (module("a", "x"), module("a", "z"), module("b", "y"));
    // `d` is a neighbour only through the reference the change removed.
    let w = module("d", "w");
    let edge = |to: &str, references| SystemMapRow::PackageEdge {
        edge: PackageEdge {
            from: "a".into(),
            to: to.into(),
            kind: PackageEdgeKind::DeclaredAndReferenced,
            references,
        },
    };
    let moved = |from: &ModuleId, to: &ModuleId, before, after| Row::ModuleReference {
        from: from.clone(),
        to: to.clone(),
        before,
        after,
    };
    let hidden = "a/src/x/hidden.rs";
    let rows = vec![
        container("a"),
        container("b"),
        container("c"),
        container("d"),
        Row::Map { row: edge("b", 10) },
        Row::Map { row: edge("c", 5) },
        moved(&x, &y, 4, 7),
        moved(&z, &y, 1, 3),
        moved(&z, &x, 2, 0),
        moved(&x, &w, 5, 0),
        moved(&y, &w, 1, 2),
        // The package's total counts a file the change deleted; no level of the head has it.
        Row::Container {
            path: "a".into(),
            added: 9,
            removed: 12,
        },
        Row::File {
            path: "a/src/x/gone.rs".into(),
            container: "a".into(),
            module: x.clone(),
            added: 0,
            removed: 10,
            binary: false,
            deleted: true,
        },
        Row::File {
            path: hidden.into(),
            container: "a".into(),
            module: x.clone(),
            added: 9,
            removed: 2,
            binary: false,
            deleted: false,
        },
    ];
    let review = review(Vec::new(), rows);
    let project = impact(&review, None).scene.unwrap();
    let change = |scene: &Scene, from: &Node, to: &Node| {
        let edge = scene.all_edges().find(|e| &e.from == from && &e.to == to);
        edge.map(|e| {
            e.change
                .iter()
                .map(|d| (d.before, d.after))
                .collect::<Vec<_>>()
        })
    };
    let (a, b, c) = (
        Node::Container("a".into()),
        Node::Container("b".into()),
        Node::Container("c".into()),
    );
    assert_eq!(change(&project, &a, &b), Some(vec![(4, 7), (1, 3)]));
    assert_eq!(change(&project, &a, &c), Some(Vec::new()));
    let d = Node::Container("d".into());
    assert_eq!(change(&project, &a, &d), Some(vec![(5, 0)]));

    let at = SystemAt::Container { path: "a".into() };
    let package = SystemMap {
        rows: vec![
            SystemMapRow::Module {
                module: x.clone(),
                stats: SystemNodeStats::default(),
            },
            SystemMapRow::Module {
                module: z.clone(),
                stats: SystemNodeStats::default(),
            },
            reference(&x, &y, 7),
            reference(&x, &z, 2),
        ],
        ..head_map(&review)
    };
    let mut scene = build(&package, Some(&at));
    changes::edges(&mut scene, &review, Some(&at));
    let port = Node::Port {
        path: "b".into(),
        into: false,
    };
    let from = Node::Module(x.clone());
    assert_eq!(change(&scene, &from, &port), Some(vec![(4, 7)]));
    assert_eq!(
        change(&scene, &from, &Node::Module(z.clone())),
        Some(Vec::new())
    );
    // A pair the head no longer has keeps an edge for its change.
    assert_eq!(change(&scene, &Node::Module(z), &from), Some(vec![(2, 0)]));
    // The head lists no port for `d`; the removed pair gets one.
    let gone = Node::Port {
        path: "d".into(),
        into: false,
    };
    assert_eq!(change(&scene, &from, &gone), Some(vec![(5, 0)]));
    assert!(scene.bottom.iter().any(|i| scene.cards[*i].node == gone));
    // A pair between two neighbours is not drawn between their ports.
    assert!(
        !scene
            .all_edges()
            .any(|e| matches!((&e.from, &e.to), (Node::Port { .. }, Node::Port { .. })))
    );

    let at = SystemAt::Module { module: x };
    let file = |path: String, lines| SystemMapRow::File {
        path,
        language: "Rust".into(),
        lines,
        test: false,
        read_status: ReadStatus::Complete,
    };
    let files = (0..=MAX_FILES)
        .map(|n| file(format!("a/src/x/f{n}.rs"), 100))
        .chain([file(hidden.into(), 10)])
        .collect();
    let level = SystemMap {
        rows: files,
        ..head_map(&review)
    };
    let scene = build(&level, Some(&at));
    let mut marks = impact(&review, None).marks;
    changes::more(&mut marks, &scene, &review, &at);
    let more = marks.get(&Node::More).copied().unwrap();
    assert_eq!((more.added, more.removed, more.files), (9, 2, 1));
    // A port's churn and count describe the same surviving files.
    let port = Node::Port {
        path: "a".into(),
        into: true,
    };
    assert_eq!(figure(&marks, &port), "+9 −2 in 1 file not listed");
    // Following an item to a folded file draws its marked "N more" card picked.
    let picked = Some(Pick::Node(Node::File(hidden.into())));
    assert_eq!(
        drawn_pick(&scene, &marks, picked),
        Some(Pick::Node(Node::More))
    );
}

#[test]
fn focus_restores_the_map_without_toggling_off_the_item() {
    let target = module_target(&module("a", "one"));
    let mut state = State {
        item: Some("row-42".into()),
        ..State::default()
    };
    state.act(Action::Focus(target.clone()));
    assert_eq!(state.item.as_deref(), Some("row-42"));
    assert_eq!(state.map.trail, target.trail);
    assert_eq!(state.map.picked, Some(Pick::Node(target.node)));
}
