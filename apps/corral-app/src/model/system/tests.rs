use corral_types::{
    ContainerFact, ContainerKind, ModuleId, PackageEdge, PackageEdgeKind, SystemMap, SystemMapRow,
    SystemNodeStats, SystemStatus, WorkspaceStatus,
};

use super::layout::{Font, layout};
use super::scene::build;
use super::*;

fn container(i: usize, lines: u64) -> SystemMapRow {
    SystemMapRow::Container {
        container: ContainerFact {
            path: format!("crates/package-number-{i}"),
            name: format!("package-number-{i}"),
            kind: ContainerKind::Cargo,
            workspace_status: WorkspaceStatus::Member,
            other_packages: Vec::new(),
        },
        stats: SystemNodeStats {
            lines,
            ..SystemNodeStats::default()
        },
    }
}

fn edge(from: usize, to: usize, kind: PackageEdgeKind) -> SystemMapRow {
    SystemMapRow::PackageEdge {
        edge: PackageEdge {
            from: format!("crates/package-number-{from}"),
            to: format!("crates/package-number-{to}"),
            kind,
            references: (from * 7 + to) as u64 % 40,
        },
    }
}

/// Thirty containers, most of them linked: a few roots using many leaves, one cycle, one
/// undeclared and one unused edge, and five with no edge at all.
fn thirty() -> SystemMap {
    let mut rows: Vec<SystemMapRow> = (0..30)
        .map(|i| container(i, 200 + (i as u64 * 937) % 40_000))
        .collect();
    for i in 0..25 {
        for j in [i + 1, i + 3, i * 2 + 1] {
            if j < 25 && j != i {
                rows.push(edge(i, j, PackageEdgeKind::DeclaredAndReferenced));
            }
        }
    }
    rows.push(edge(20, 2, PackageEdgeKind::ReferencedOnly));
    rows.push(edge(4, 23, PackageEdgeKind::DeclaredOnly));
    SystemMap {
        project: "p".into(),
        head: Some("abc".into()),
        status: SystemStatus::Ready,
        error: None,
        provider: None,
        coverage: None,
        omitted_rows: 0,
        rows,
    }
}

fn estimate(font: Font, text: &str) -> f32 {
    font.estimate(text)
}

/// The acceptance case: thirty containers in the room a 1440-wide window leaves beside the
/// sidebar and the inspector must not overlap, overflow or leave an edge unrouted.
#[test]
fn thirty_containers_fit_without_overlap() {
    let scene = build(&thirty(), None);
    assert_eq!(scene.graph.len(), 25);
    assert_eq!(scene.band.len(), 5);
    let room = 860.0;
    // The text engine's fonts run wider than the estimate; both have to fit.
    let wide = |font: Font, text: &str| font.estimate(text) * 1.5;
    for measure in [&estimate as &dyn Fn(Font, &str) -> f32, &wide] {
        let placed = layout(&scene, room, measure);
        assert_eq!(placed.placed.len(), 30);
        for (i, a) in placed.placed.iter().enumerate() {
            assert!(
                a.rect.x >= 0.0 && a.rect.x + a.rect.w <= room + 0.5,
                "{:?} leaves the room",
                a.rect
            );
            for b in &placed.placed[i + 1..] {
                assert!(
                    !a.rect.overlaps(&b.rect),
                    "{:?} overlaps {:?}",
                    a.rect,
                    b.rect
                );
            }
        }
        assert_eq!(placed.routes.len(), scene.edges.len());
        assert!(placed.routes.iter().all(|r| r.points.len() >= 2));
    }
}

/// A name the card cut would break the "no clipped names" promise on the map.
#[test]
fn cards_are_wide_enough_for_their_names() {
    let scene = build(&thirty(), None);
    for card in &scene.cards {
        let (w, _) = layout::size(card, &scene, &estimate);
        let tag = card
            .tag
            .as_deref()
            .map_or(0.0, |t| estimate(Font::Tag, t) + 14.0);
        assert!(
            w >= estimate(Font::Name, &card.name) + tag + 24.0,
            "{}",
            card.name
        );
    }
}

#[test]
fn selecting_again_drills_in_and_escape_comes_back_to_it() {
    let module = ModuleId {
        container: "crates/a".into(),
        path: "src".into(),
    };
    let mut state = State::default();
    let a = Node::Container("crates/a".into());
    state.act(Action::Pick(Pick::Node(a.clone())));
    assert!(state.trail.is_empty());
    state.act(Action::Pick(Pick::Node(a.clone())));
    assert_eq!(state.trail.len(), 1);
    state.act(Action::Open(Node::Module(module)));
    assert_eq!(state.trail.len(), 2);
    state.act(Action::To(1));
    assert_eq!(
        state.picked,
        Some(Pick::Node(Node::Module(ModuleId {
            container: "crates/a".into(),
            path: "src".into()
        })))
    );
    state.act(Action::Up);
    assert!(state.trail.is_empty());
    assert_eq!(state.picked, Some(Pick::Node(a)));
    // A neighbouring package opens as its own package, not under this one.
    state.act(Action::Open(Node::Container("crates/a".into())));
    state.act(Action::Open(Node::Port {
        path: "crates/b".into(),
        into: false,
    }));
    assert_eq!(
        state.trail,
        vec![SystemAt::Container {
            path: "crates/b".into()
        }]
    );
}

fn runtime_fixture() -> corral_types::SystemRuntime {
    use corral_types::{
        EntryKind, EntryPoint, RuntimeBasis, RuntimeEdge, RuntimeEdgeKind, RuntimeSite,
        RuntimeSource, RuntimeUnit, RuntimeUnitId, RuntimeUnitKind, SystemRuntimeRow as Row,
    };
    let id = |kind, name: &str, path: &str| RuntimeUnitId {
        kind,
        name: name.into(),
        path: path.into(),
    };
    let unit = |id: RuntimeUnitId| Row::Unit {
        unit: RuntimeUnit {
            id,
            line: 3,
            container: "a".into(),
            command: None,
            entry_point: None,
            image: None,
            build_context: None,
            dockerfile: None,
            ports: Vec::new(),
            depends_on: Vec::new(),
        },
    };
    let bin = id(RuntimeUnitKind::Binary, "tool", "a/src/main.rs");
    let starts = |line| Row::Edge {
        edge: RuntimeEdge {
            kind: RuntimeEdgeKind::Starts,
            from: RuntimeSource::Container { path: "b".into() },
            to: bin.clone(),
            basis: RuntimeBasis::Syntax,
            site: RuntimeSite {
                path: "b/src/run.rs".into(),
                line,
            },
        },
    };
    let mut rows = vec![
        unit(bin.clone()),
        unit(id(RuntimeUnitKind::Job, "ci", ".github/workflows/ci.yml")),
        starts(10),
        starts(20),
        Row::Entry {
            entry: EntryPoint {
                kind: EntryKind::Command,
                name: "serve".into(),
                path: "a/src/cli.rs".into(),
                line: 4,
                owner: RuntimeSource::Unit { id: bin.clone() },
            },
        },
        Row::EntryCoverage {
            container: "c".into(),
            detectors: Default::default(),
        },
    ];
    for name in ["dev", "build", "test"] {
        rows.push(unit(id(RuntimeUnitKind::Script, name, "a/package.json")));
    }
    corral_types::SystemRuntime {
        project: "p".into(),
        head: Some("abc".into()),
        status: SystemStatus::Ready,
        error: None,
        omitted_rows: 0,
        rows,
    }
}

/// The Runtime view on the map's own code: scripts fold until the fold is selected again, two
/// sites of one pair are one edge with both lines, an entry point sits under its unit, a
/// package no detector applies to says so, and connected units are banded by kind like the
/// rest (a connected binary was left in an unbanded graph), without overlap.
#[test]
fn runtime_folds_scripts_merges_sites_and_bands_every_kind() {
    let runtime = runtime_fixture();
    let mut state = State {
        view: View::Runtime,
        ..State::default()
    };
    let scene = runtime::build(&runtime, state.scripts);
    let scripts = scene.card(&Node::Scripts).map(|c| c.folded.len());
    assert_eq!(scripts, Some(3));
    assert_eq!(scene.edges.len(), 1);
    assert_eq!(scene.edges[0].sites.len(), 2);
    let tool = scene.cards.iter().find(|c| c.name == "tool");
    assert_eq!(tool.map(|c| c.entries.len()), Some(1));
    let c = scene.card(&Node::Container("c".into()));
    assert_eq!(c.map(runtime::figure).as_deref(), Some("no detector"));
    let placed = layout(&scene, 600.0, &estimate);
    assert_eq!(placed.placed.len(), scene.cards.len());
    for (i, a) in placed.placed.iter().enumerate() {
        for b in &placed.placed[i + 1..] {
            assert!(
                !a.rect.overlaps(&b.rect),
                "{:?} overlaps {:?}",
                a.rect,
                b.rect
            );
        }
    }
    let top_of = |name: &str| {
        let card = scene.cards.iter().position(|c| c.name == name);
        let y = card.and_then(|c| placed.rect(c)).map(|r| r.y);
        y.unwrap_or_else(|| panic!("{name} is not placed"))
    };
    let captions: Vec<&str> = placed.captions.iter().map(|c| c.text.as_str()).collect();
    for title in ["Packages and folders", "CI jobs", "Binaries", "npm scripts"] {
        assert!(captions.contains(&title), "{title} in {captions:?}");
    }
    assert!(
        top_of("ci") < top_of("tool"),
        "the job's band sits above the binaries'"
    );
    assert!(top_of("b") < top_of("ci"), "the packages' band sits on top");
    state.act(Action::Pick(Pick::Node(Node::Scripts)));
    state.act(Action::Pick(Pick::Node(Node::Scripts)));
    assert!(state.scripts && state.trail.is_empty());
    let unfolded = runtime::build(&runtime, state.scripts);
    assert!(unfolded.card(&Node::Scripts).is_none());
    assert_eq!(unfolded.cards.len(), scene.cards.len() + 2);
}

fn data_fixture() -> corral_types::SystemData {
    use corral_types::{
        DataEdge, DataEdgeKind, DataField, DataKind, DataSource, DataType, DataTypeId,
        SystemDataRow,
    };
    let id = |path: &str| DataTypeId {
        path: path.into(),
        name: path.rsplit('/').next().unwrap_or_default().into(),
    };
    let ty = |path: &str, fields: &[&str]| SystemDataRow::Type {
        data_type: DataType {
            path: path.into(),
            name: id(path).name,
            line: 1,
            kind: DataKind::Record,
            source: DataSource::RustSerde,
            fields: fields
                .iter()
                .enumerate()
                .map(|(i, f)| DataField {
                    name: (*f).into(),
                    ty: "T".into(),
                    line: 2 + i as u64,
                    optional: false,
                })
                .collect(),
        },
    };
    let container = |path: &str, language: &str| SystemDataRow::Container {
        path: path.into(),
        name: path.into(),
        languages: [language.to_owned()].into(),
    };
    let has = |from: &str, to: &str, field: &str| SystemDataRow::Has {
        edge: DataEdge {
            kind: DataEdgeKind::Has,
            from: id(from),
            to: id(to),
            field: field.into(),
            line: 2,
        },
    };
    corral_types::SystemData {
        project: "p".into(),
        commit: Some("abc".into()),
        status: SystemStatus::Ready,
        error: None,
        provider: None,
        omitted_rows: 0,
        rows: vec![
            container("a", "rust"),
            container("b", "rust"),
            container("web", "typescript"),
            SystemDataRow::Coverage {
                language: "rust".into(),
                sources: [DataSource::RustSerde].into(),
            },
            SystemDataRow::Coverage {
                language: "typescript".into(),
                sources: Default::default(),
            },
            ty("a/src/Task", &["run", "id"]),
            ty("b/src/Run", &["check"]),
            ty("b/src/Check", &[]),
            has("a/src/Task", "b/src/Run", "run"),
            has("b/src/Run", "b/src/Check", "check"),
        ],
    }
}

/// The Data view groups types in their containers' bands, draws a has edge only where both
/// types are drawn (an edge to a filtered-out type has no card to reach) while the inspector
/// still names the holder, and says a container no source reads is not provided, never empty.
#[test]
fn data_view_bands_by_container_and_keeps_links_past_the_filter() {
    let data = data_fixture();
    let scene = data::build(&data, "");
    assert_eq!(scene.edges.len(), 2);
    let placed = layout(&scene, 600.0, &estimate);
    let captions: Vec<&str> = placed.captions.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(captions, ["a", "b"]);
    let filtered = data::build(&data, "check");
    assert_eq!(filtered.cards.len(), 1);
    assert!(filtered.edges.is_empty());
    let check = Node::Type(corral_types::DataTypeId {
        path: "b/src/Check".into(),
        name: "Check".into(),
    });
    let inspected = inspect::inspect(&filtered, &Pick::Node(check), None);
    let held = inspected
        .iter()
        .flat_map(|i| &i.sections)
        .find(|s| s.title.as_deref().is_some_and(|t| t.starts_with("Held by")))
        .map(|s| (s.rows.len(), s.rows[0].pick.is_none()));
    assert_eq!(
        held,
        Some((1, true)),
        "Run holds Check though the filter hides Run"
    );
    let summary = data::summary(&data).join(" | ");
    assert!(summary.contains("Not provided for typescript"), "{summary}");
    assert!(summary.contains("web"), "{summary}");
}
