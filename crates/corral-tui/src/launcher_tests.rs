use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use crate::HitMap;
use crate::hit::press;
use crate::launcher::{LauncherEntry, Section, State, Step, draw};
use crate::theme::SELECTED_BG;

fn start(id: &str, key: char, name: &str, detail: &str) -> LauncherEntry {
    LauncherEntry {
        id: id.into(),
        section: Section::Start,
        key: Some(key),
        name: name.into(),
        detail: detail.into(),
        options: Vec::new(),
    }
}

fn resume(id: &str, detail: &str) -> LauncherEntry {
    LauncherEntry {
        id: id.into(),
        section: Section::Resume,
        key: None,
        name: id.into(),
        detail: detail.into(),
        options: Vec::new(),
    }
}

fn entries() -> Vec<LauncherEntry> {
    vec![
        start(
            "assistant",
            'a',
            "Assistant",
            "board-aware · scratch worktrees",
        ),
        start("pm", 'p', "PM", "workspace coordinator"),
        resume("A-31", "Cache spike · corral · 2h ago"),
        resume("A-29", "Arrival layout notes · mitiplay · 1d ago"),
    ]
}

/// [`entries`] under a first Build row with two options.
fn with_build() -> Vec<LauncherEntry> {
    let mut build = start("build", 'b', "Build", "untethered · parallel agents");
    build.options = vec![
        "new worktree  .worktrees/build-<name>".into(),
        "here  the current checkout and branch".into(),
    ];
    std::iter::once(build).chain(entries()).collect()
}

/// The launcher column starts here at both test widths: a two-column margin, centered.
const COLUMN_X: u16 = 2;

fn render(width: u16, list: &[LauncherEntry], state: &mut State) -> Vec<(String, bool)> {
    let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
    let mut hits = HitMap::default();
    terminal.draw(|f| draw(f, list, state, &mut hits)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            let text: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            (
                text.trim_end().to_string(),
                buffer[(COLUMN_X, y)].bg == SELECTED_BG,
            )
        })
        .collect()
}

fn row(rows: &[(String, bool)], text: &str) -> usize {
    rows.iter()
        .position(|(line, _)| line.contains(text))
        .unwrap_or_else(|| panic!("no row shows {text:?}: {rows:#?}"))
}

#[test]
fn arrows_and_vim_keys_cross_from_start_into_resume_and_back() {
    let list = entries();
    let mut state = State::default();
    state.key(&list, press(KeyCode::Down));
    assert_eq!(state.selected, 1, "PM is the last Start row");
    assert_eq!(state.key(&list, press(KeyCode::Char('j'))), Step::Stay);
    assert_eq!(state.selected, 2, "the first Resume row follows it");
    state.key(&list, press(KeyCode::Char('k')));
    assert_eq!(state.selected, 1);
    state.key(&list, press(KeyCode::Up));
    state.key(&list, press(KeyCode::Up));
    assert_eq!(state.selected, 0, "the selection stops at the first row");
}

#[test]
fn a_row_key_opens_its_row_and_enter_opens_the_selection() {
    let list = entries();
    let mut state = State::default();
    assert_eq!(state.key(&list, press(KeyCode::Char('p'))), Step::Open(1));
    assert_eq!(state.selected, 1);
    state.key(&list, press(KeyCode::Down));
    assert_eq!(state.key(&list, press(KeyCode::Enter)), Step::Open(2));
}

#[test]
fn esc_q_and_ctrl_c_leave_without_opening() {
    let list = entries();
    for key in [
        press(KeyCode::Esc),
        press(KeyCode::Char('q')),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        assert_eq!(State::default().key(&list, key), Step::Leave, "{key:?}");
    }
}

#[test]
fn a_click_selects_and_a_second_click_opens() {
    let mut state = State::default();
    assert_eq!(state.click(2), Step::Stay);
    assert_eq!(state.selected, 2);
    assert_eq!(state.click(2), Step::Open(2));
}

#[test]
fn wide_rows_keep_name_and_detail_on_one_line_with_the_marker_on_the_selection() {
    let mut state = State::default();
    let rows = render(100, &entries(), &mut state);
    let assistant = row(&rows, "Assistant");
    assert!(rows[assistant].0.contains("▌ Assistant"), "{rows:#?}");
    assert!(rows[assistant].0.contains("board-aware"), "{rows:#?}");
    assert!(
        rows[assistant].1,
        "the selected row has the selected background"
    );
    let pm = row(&rows, "PM ");
    assert!(!rows[pm].0.contains('▌') && !rows[pm].1, "{rows:#?}");
    assert!(row(&rows, "Start") < pm && pm < row(&rows, "Resume"));
    assert!(row(&rows, "Resume") < row(&rows, "A-31"));
    assert!(
        rows[row(&rows, "A-31")]
            .0
            .contains("Cache spike · corral · 2h ago")
    );
}

#[test]
fn narrow_rows_fold_to_name_then_detail_with_the_marker_on_the_selection() {
    let mut state = State::default();
    state.key(&entries(), press(KeyCode::Char('j')));
    state.key(&entries(), press(KeyCode::Char('j')));
    let rows = render(60, &entries(), &mut state);
    let resumed = row(&rows, "A-31");
    assert!(rows[resumed].0.contains("▌ A-31"), "{rows:#?}");
    assert!(!rows[resumed].0.contains("Cache spike"), "{rows:#?}");
    assert!(rows[resumed + 1].0.contains("Cache spike"), "{rows:#?}");
    assert!(
        rows[resumed].1 && rows[resumed + 1].1,
        "both lines are selected"
    );
    let assistant = row(&rows, "Assistant");
    assert!(
        !rows[assistant].0.contains('▌') && !rows[assistant].1,
        "{rows:#?}"
    );
    assert!(rows[assistant + 1].0.contains("board-aware"), "{rows:#?}");
}

#[test]
fn left_and_right_cycle_the_option_only_on_a_row_that_has_options() {
    let list = with_build();
    let mut state = State::default();
    assert_eq!(state.key(&list, press(KeyCode::Right)), Step::Stay);
    assert_eq!(state.option(0), 1);
    state.key(&list, press(KeyCode::Right));
    assert_eq!(state.option(0), 0, "the options wrap");
    state.key(&list, press(KeyCode::Left));
    assert_eq!(state.option(0), 1, "and wrap backwards");
    state.key(&list, press(KeyCode::Down));
    for code in [KeyCode::Left, KeyCode::Right] {
        assert_eq!(state.key(&list, press(code)), Step::Stay);
    }
    assert_eq!(state.selected, 1, "←→ never move the selection");
    assert_eq!(state.option(1), 0, "Assistant has no options to cycle");
    assert_eq!(state.option(0), 1, "Build keeps its choice");
}

#[test]
fn the_opened_row_carries_its_chosen_option() {
    let list = with_build();
    let mut state = State::default();
    state.key(&list, press(KeyCode::Left));
    state.key(&list, press(KeyCode::Down));
    assert_eq!(state.key(&list, press(KeyCode::Char('b'))), Step::Open(0));
    assert_eq!(state.picked(&list, 0), Some(("build".into(), 1)));
    assert_eq!(state.picked(&list, 1), Some(("assistant".into(), 0)));
    assert_eq!(state.picked(&list, list.len()), None);
}

#[test]
fn the_option_line_shows_only_under_the_selected_build_row_at_both_widths() {
    for width in [100, 60] {
        let list = with_build();
        let mut state = State::default();
        let rows = render(width, &list, &mut state);
        let build = row(&rows, "▌ Build");
        let option = row(&rows, "‹ new worktree  .worktrees/build-<name> ›");
        let label = row(&rows, "where it writes");
        assert!(
            build < label && label <= option && option <= build + 3,
            "{width}: {rows:#?}"
        );
        assert!(rows[label].1, "{width}: the label is on the selection");
        assert!(rows[option].1, "{width}: the option is on the selection");
        assert!(option < row(&rows, "Assistant"), "{width}: {rows:#?}");
        let footer = &rows[rows.len() - 1].0;
        assert!(footer.contains("←→ where it writes"), "{width}: {footer}");
        state.key(&list, press(KeyCode::Right));
        let rows = render(width, &list, &mut state);
        row(&rows, "‹ here  the current checkout and branch ›");
        state.key(&list, press(KeyCode::Down));
        let rows = render(width, &list, &mut state);
        let shown = |text: &str| rows.iter().any(|(line, _)| line.contains(text));
        assert!(
            !shown("‹") && !shown("where it writes"),
            "{width}: no option line or hint once Assistant is selected: {rows:#?}"
        );
    }
}
