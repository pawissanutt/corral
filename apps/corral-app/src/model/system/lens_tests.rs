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
    };
    let Some(d) = diagram(&result, None, &[], &touch) else {
        panic!("a readable lens draws");
    };
    assert!(d.edges[0].marks.is_empty());
    assert_eq!(d.edges[1].marks, vec![Mark::Failed]);
    assert_eq!(d.nodes[0].marks, vec![Mark::Touched]);
    assert_eq!(d.nodes[1].marks, vec![Mark::Extra]);
    assert_eq!(d.failures[0].what, "Transition retry");
}
