use corral_types::{
    ContainerFact, ContainerKind, ModuleId, PackageEdge, PackageEdgeKind, SystemMap,
    SystemMapRow, SystemNodeStats, SystemStatus, WorkspaceStatus,
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
    let mut rows: Vec<SystemMapRow> = (0..30).map(|i| container(i, 200 + (i as u64 * 937) % 40_000)).collect();
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
    let placed = layout(&scene, room, &estimate);
    assert_eq!(placed.placed.len(), 30);
    for (i, a) in placed.placed.iter().enumerate() {
        assert!(a.rect.x >= 0.0 && a.rect.x + a.rect.w <= room + 0.5, "{:?} leaves the room", a.rect);
        for b in &placed.placed[i + 1..] {
            assert!(!a.rect.overlaps(&b.rect), "{:?} overlaps {:?}", a.rect, b.rect);
        }
    }
    assert_eq!(placed.routes.len(), scene.edges.len());
    assert!(placed.routes.iter().all(|r| r.points.len() >= 2));
}

/// A name the card cut would break the "no clipped names" promise on the map.
#[test]
fn cards_are_wide_enough_for_their_names() {
    let scene = build(&thirty(), None);
    for card in &scene.cards {
        let (w, _) = layout::size(card, &scene, &estimate);
        let tag = card.tag.as_deref().map_or(0.0, |t| estimate(Font::Tag, t) + 14.0);
        assert!(w >= estimate(Font::Name, &card.name) + tag + 24.0, "{}", card.name);
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
    assert_eq!(state.picked, Some(Pick::Node(Node::Module(ModuleId { container: "crates/a".into(), path: "src".into() }))));
    state.act(Action::Up);
    assert!(state.trail.is_empty());
    assert_eq!(state.picked, Some(Pick::Node(a)));
    // A neighbouring package opens as its own package, not under this one.
    state.act(Action::Open(Node::Container("crates/a".into())));
    state.act(Action::Open(Node::Port { path: "crates/b".into(), into: false }));
    assert_eq!(state.trail, vec![SystemAt::Container { path: "crates/b".into() }]);
}
