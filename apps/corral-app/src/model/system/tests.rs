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

fn sample_level(at: SystemAt) -> SystemMap {
    use crate::model::{daemon::Feed, preview};
    use corral_protocol::detail::{DetailBody, DetailRef};

    let target = DetailRef::SystemMap {
        project: "corral".into(),
        at: Some(at),
    };
    let mut feed = Feed::default();
    preview::detail(&mut feed, &target, None).unwrap();
    match feed.detail(&target).and_then(|cache| cache.body()) {
        Some(DetailBody::SystemMap(map)) => map.clone(),
        _ => panic!("sample map not served"),
    }
}

/// The sample's package is a chain of ranks one module wide; when several fit beside
/// one another, the chain should not take a whole screen of height and leave most width empty.
#[test]
fn package_chain_uses_width_without_losing_routes() {
    let at = SystemAt::Container {
        path: "apps/corral-app".into(),
    };
    let scene = build(&sample_level(at.clone()), Some(&at));
    for room in [540.0, 1200.0] {
        let placed = layout(&scene, room, &estimate);
        let modules: Vec<_> = scene
            .graph
            .iter()
            .map(|&i| placed.rect(i).unwrap())
            .collect();
        let left = modules.iter().map(|r| r.x).fold(f32::INFINITY, f32::min);
        let right = modules.iter().map(|r| r.x + r.w).fold(0.0, f32::max);
        assert!(right - left > room / 2.0, "{modules:?}");
        assert!(placed.width <= room + 0.5);
        assert_eq!(placed.routes.len(), scene.edges.len());
        for (i, p) in placed.placed.iter().enumerate() {
            assert!(
                placed.placed[i + 1..]
                    .iter()
                    .all(|q| !p.rect.overlaps(&q.rect))
            );
        }
        for route in &placed.routes {
            let edge = &scene.edges[route.edge];
            for (node, &(x, y)) in [
                (&edge.from, route.points.first().unwrap()),
                (&edge.to, route.points.last().unwrap()),
            ] {
                let i = scene.cards.iter().position(|c| &c.node == node).unwrap();
                let r = placed.rect(i).unwrap();
                assert!(x >= r.x && x <= r.x + r.w);
                assert!((y - r.y).abs() < 0.01 || (y - r.y - r.h).abs() < 0.01);
            }
            assert!(route.points.iter().all(|&(x, y)| {
                x >= 0.0 && x <= placed.width && y >= 0.0 && y <= placed.height
            }));
        }
    }
}

/// The standalone file band had no caption, and its count must include the files it folds.
#[test]
fn file_grid_caption_counts_folded_files() {
    let module = ModuleId {
        container: "apps/corral-app".into(),
        path: "model".into(),
    };
    let at = SystemAt::Module {
        module: module.clone(),
    };
    let map = sample_level(at.clone());
    let scene = build(&map, Some(&at));
    let placed = layout(&scene, 800.0, &estimate);
    let [caption] = placed.captions.as_slice() else {
        panic!("file band missing its caption");
    };
    let file_count = map.rows.len() as u64;
    let lines = map
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemMapRow::File { lines, .. } => Some(lines),
            _ => None,
        })
        .sum();
    assert!(caption.text.contains(&module.container) && caption.text.contains(&module.path));
    let numbers: Vec<&str> = caption
        .text
        .split_whitespace()
        .filter(|word| word.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert_eq!(numbers, [count(file_count), count(lines)]);
    assert!(placed.placed.iter().all(|p| p.rect.y > caption.y));
    assert_eq!(
        scene.card(&Node::More).unwrap().more.unwrap().0,
        file_count - scene::MAX_FILES as u64
    );
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
    let container = |path: &str, language: &str, sources: &[DataSource]| SystemDataRow::Container {
        path: path.into(),
        name: path.into(),
        languages: [language.to_owned()].into(),
        sources: sources.iter().copied().collect(),
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
            container("a", "rust", &[DataSource::RustSerde]),
            container("b", "rust", &[DataSource::RustSerde]),
            container("ext", "typescript", &[DataSource::Extension]),
            container("web", "typescript", &[]),
            SystemDataRow::Coverage {
                language: "rust".into(),
                sources: [DataSource::RustSerde].into(),
            },
            SystemDataRow::Coverage {
                language: "typescript".into(),
                sources: [DataSource::Extension].into(),
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
/// still names the holder, and says a container no source read is not provided, never empty,
/// even when its language is read in another container.
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
    let unprovided: Vec<String> = data::containers(&data)
        .into_iter()
        .filter(|c| !data::container_provided(c))
        .map(|c| c.path)
        .collect();
    assert_eq!(unprovided, ["web"]);
}

/// No types read is three different answers: nothing reads the project's languages (not
/// provided), a scan that read them found none (empty), and a reply still building or without
/// coverage (not known). Only the second may say the project has no data types.
#[test]
fn no_types_is_empty_only_after_a_covered_scan() {
    use corral_types::{DataSource, SystemDataRow};
    use data::Outcome;
    let served = |status, rows: Vec<SystemDataRow>| corral_types::SystemData {
        project: "p".into(),
        commit: Some("abc".into()),
        status,
        error: None,
        provider: None,
        omitted_rows: 0,
        rows,
    };
    let coverage = |language: &str, sources: &[DataSource]| SystemDataRow::Coverage {
        language: language.into(),
        sources: sources.iter().copied().collect(),
    };
    let typescript_only = served(SystemStatus::Ready, vec![coverage("TypeScript", &[])]);
    assert_eq!(data::outcome(&typescript_only), Outcome::NotProvided);
    let rust_without_serde = served(
        SystemStatus::Ready,
        vec![coverage("Rust", &[DataSource::RustSerde])],
    );
    assert_eq!(data::outcome(&rust_without_serde), Outcome::Empty);
    assert_eq!(
        data::outcome(&served(SystemStatus::Building, Vec::new())),
        Outcome::Pending
    );
    assert_eq!(
        data::outcome(&served(SystemStatus::Ready, Vec::new())),
        Outcome::Pending
    );
    let unread = served(
        SystemStatus::Ready,
        vec![
            coverage("Rust", &[DataSource::RustSerde]),
            SystemDataRow::Unreadable {
                path: "a.rs".into(),
            },
        ],
    );
    assert_eq!(data::outcome(&unread), Outcome::Incomplete);
}
