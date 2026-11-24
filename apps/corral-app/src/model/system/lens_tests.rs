use corral_types::lenses::{
    Lens, LensAnchor, LensDiagnostic, LensNode, LensResult, LensStatus, LensTransition, StatesFrom,
};

use super::*;

fn anchor() -> LensAnchor {
    LensAnchor {
        path: "src/lib.rs".into(),
        name: None,
        text: None,
    }
}

fn node(id: &str) -> LensNode {
    LensNode {
        id: id.into(),
        label: id.to_uppercase(),
        anchor: anchor(),
    }
}

fn retry() -> LensTransition {
    LensTransition {
        from: "a".into(),
        to: "b".into(),
        label: "retry".into(),
        guard: None,
        anchor: anchor(),
    }
}

/// The checker numbers a repeated transition from its second occurrence; a failure on the
/// second must mark that one, not the first, or the owner repairs the wrong arrow.
#[test]
fn a_failure_on_a_repeated_transition_marks_that_repeat() {
    let lens = Lens::State {
        id: "l".into(),
        title: "L".into(),
        read_at: "c".into(),
        states_from: StatesFrom {
            path: "src/lib.rs".into(),
            name: "S".into(),
        },
        states: vec![node("a"), node("b")],
        transitions: vec![retry(), retry()],
    };
    let result = LensResult {
        file: ".corral/lenses/l.toml".into(),
        lens: Some(lens),
        status: LensStatus::Stale,
        failures: vec![LensDiagnostic {
            element: r#"transition "a" -> "b": "retry" (#2)"#.into(),
            message: "text not found".into(),
        }],
        not_checked: Vec::new(),
        locations: Vec::new(),
        missing_states: Vec::new(),
        extra_states: vec!["b".into()],
    };
    let touch = Touch {
        touched: vec!["state a".into()],
        added: Vec::new(),
        removed: Vec::new(),
    };
    let Some(d) = diagram(&result, None, &[], &touch) else {
        panic!("a readable lens draws");
    };
    assert!(d.edges[0].marks.is_empty());
    assert_eq!(d.edges[1].marks, vec![Mark::Failed]);
    assert_eq!(d.nodes[0].marks, vec![Mark::Touched]);
    assert_eq!(d.nodes[1].marks, vec![Mark::Extra]);
}

/// Transitions between the same two states share one layered route. Each must take its own
/// lane, or the later stroke hides the earlier one and its mark, and each must still leave and
/// reach its states: shifting whole routes put the eighth of eight 56pt off an 84pt state.
#[test]
fn parallel_transitions_take_separate_lanes_and_stay_on_their_states() {
    let mut transitions: Vec<LensTransition> = (0..8)
        .map(|n| LensTransition {
            label: format!("t{n}"),
            ..retry()
        })
        .collect();
    transitions.push(LensTransition {
        from: "b".into(),
        to: "a".into(),
        ..retry()
    });
    let lens = Lens::State {
        id: "l".into(),
        title: "L".into(),
        read_at: "c".into(),
        states_from: StatesFrom {
            path: "src/lib.rs".into(),
            name: "S".into(),
        },
        states: vec![node("a"), node("b")],
        transitions,
    };
    let result = LensResult {
        file: "l.toml".into(),
        lens: Some(lens),
        status: LensStatus::Current,
        failures: Vec::new(),
        not_checked: Vec::new(),
        locations: Vec::new(),
        missing_states: Vec::new(),
        extra_states: Vec::new(),
    };
    let Some(d) = diagram(&result, None, &[], &Touch::default()) else {
        panic!("a readable lens draws");
    };
    let measure = |font: crate::model::system::layout::Font, words: &str| font.estimate(words);
    let placed = crate::model::system::lens_layout::layout(&d, 800.0, &measure);
    let boxes: Vec<_> = placed.nodes.iter().map(|n| n.rect).collect();
    let on_a_state = |(x, y): (f32, f32)| {
        boxes
            .iter()
            .any(|r| x >= r.x && x <= r.x + r.w && y >= r.y - 1.0 && y <= r.y + r.h + 1.0)
    };
    assert_eq!(placed.arrows.len(), 9);
    let mut mids = Vec::new();
    for arrow in &placed.arrows {
        let (Some(&first), Some(&last)) = (arrow.points.first(), arrow.points.last()) else {
            panic!("an arrow has ends");
        };
        assert!(on_a_state(first) && on_a_state(last), "{:?}", arrow.points);
        let bends = &arrow.points[1..arrow.points.len() - 1];
        mids.push(bends.iter().map(|p| p.0).sum::<f32>() / bends.len() as f32);
    }
    for (i, a) in mids.iter().enumerate() {
        for b in &mids[i + 1..] {
            assert!((a - b).abs() > 4.0, "{mids:?}");
        }
    }
}
