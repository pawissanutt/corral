/// Transitions between the same two states share one layered route. Each must take its own
/// lane, or the later stroke hides the earlier one and its mark, and each must still leave and
/// reach its states: shifting whole routes put the eighth of eight 56pt off an 84pt state.
#[test]
fn parallel_transitions_take_separate_lanes_and_stay_on_their_states() {
    let mut source = String::from("flowchart LR\n");
    for n in 0..8 {
        source.push_str(&format!("a -->|t{n}| b\n"));
    }
    source.push_str("b --> a");
    let d = crate::model::artifacts::mermaid::parse(&source).unwrap();
    let measure = |font: crate::model::system::layout::Font, words: &str| font.estimate(words);
    let placed = crate::model::system::lens_layout::layout(&d, 800.0, &measure);
    let on = |node: usize, (x, y): (f32, f32)| {
        placed.nodes.iter().any(|n| {
            let r = n.rect;
            n.node == node && x >= r.x && x <= r.x + r.w && y >= r.y - 1.0 && y <= r.y + r.h + 1.0
        })
    };
    assert_eq!(placed.arrows.len(), 9);
    let mut mids = Vec::new();
    for arrow in &placed.arrows {
        let (Some(&first), Some(&last)) = (arrow.points.first(), arrow.points.last()) else {
            panic!("an arrow has ends");
        };
        let Some((from, to)) = d.edges[arrow.edge].ends else {
            panic!("a transition has ends");
        };
        // It leaves its own source and reaches its own target, inside the canvas.
        assert!(on(from, first) && on(to, last), "{:?}", arrow.points);
        let inside = arrow
            .points
            .iter()
            .all(|p| p.0 >= 0.0 && p.0 <= placed.width);
        assert!(inside, "{:?} in {}", arrow.points, placed.width);
        let bends = &arrow.points[1..arrow.points.len() - 1];
        mids.push(bends.iter().map(|p| p.0).sum::<f32>() / bends.len() as f32);
    }
    for (i, a) in mids.iter().enumerate() {
        for b in &mids[i + 1..] {
            assert!((a - b).abs() > 4.0, "{mids:?}");
        }
    }
}

/// Repeated self-transitions grow their loops up and down; on a first-row state the fifth
/// bent above the canvas and was clipped.
#[test]
fn repeated_self_transitions_stay_inside_the_canvas() {
    let mut source = String::from("flowchart LR\n");
    for n in 0..5 {
        source.push_str(&format!("a -->|t{n}| a\n"));
    }
    let d = crate::model::artifacts::mermaid::parse(&source).unwrap();
    let measure = |font: crate::model::system::layout::Font, words: &str| font.estimate(words);
    let placed = crate::model::system::lens_layout::layout(&d, 800.0, &measure);
    assert_eq!(placed.arrows.len(), 5);
    for arrow in &placed.arrows {
        let inside = arrow
            .points
            .iter()
            .all(|p| (0.0..=placed.width).contains(&p.0) && (0.0..=placed.height).contains(&p.1));
        assert!(
            inside,
            "{:?} in {}x{}",
            arrow.points, placed.width, placed.height
        );
    }
}
