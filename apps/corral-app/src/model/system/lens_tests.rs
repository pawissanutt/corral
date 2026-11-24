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

/// A transition and its reverse share one layered route; drawn on it, the later stroke hides
/// the earlier one and its failing mark.
#[test]
fn transitions_between_the_same_states_take_separate_lanes() {
    let back = LensTransition {
        from: "b".into(),
        to: "a".into(),
        ..retry()
    };
    let lens = Lens::State {
        id: "l".into(),
        title: "L".into(),
        read_at: "c".into(),
        states_from: StatesFrom {
            path: "src/lib.rs".into(),
            name: "S".into(),
        },
        states: vec![node("a"), node("b")],
        transitions: vec![retry(), back],
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
    let mids: Vec<f32> = placed
        .arrows
        .iter()
        .map(|a| a.points.iter().map(|p| p.0).sum::<f32>() / a.points.len() as f32)
        .collect();
    assert_eq!(mids.len(), 2);
    assert!((mids[0] - mids[1]).abs() > 4.0, "{mids:?}");
}
