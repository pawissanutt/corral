use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use crate::hit::press;
use crate::launcher::{
    ACCENT, Billing, KEY, Launch, LauncherContext, LauncherEntry, LauncherProfile, Login, OK,
    ProviderLogin, SELECTED, Section, State, Step, WARN, draw,
};
use crate::{Hit, HitMap};

const PLACE: &str = "~/miti-workspace/mitiplay";

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

/// The launcher column starts here at every width under 106 columns: one cell in from the edge.
const COLUMN_X: u16 = 1;

/// One drawn frame: each row's text with whether the launcher column's first cell is on the
/// selected surface, the cells, and the mouse regions.
struct Screen {
    rows: Vec<(String, bool)>,
    buffer: Buffer,
    hits: HitMap,
}

impl Screen {
    fn row(&self, text: &str) -> usize {
        row(&self.rows, text)
    }

    /// The column where `text` starts on row `y` (every glyph the launcher draws is one cell).
    fn col(&self, y: usize, text: &str) -> u16 {
        let line = &self.rows[y].0;
        let at = line
            .find(text)
            .unwrap_or_else(|| panic!("row {y} has no {text:?}: {line:?}"));
        line[..at].chars().count() as u16
    }

    fn hit(&self, x: u16, y: usize) -> Option<&Hit> {
        self.hits.at(x, y as u16)
    }
}

fn screen(width: u16, height: u16, list: &[LauncherEntry], state: &mut State) -> Screen {
    screen_with(width, height, list, &LauncherContext::default(), state)
}

fn screen_with(
    width: u16,
    height: u16,
    list: &[LauncherEntry],
    context: &LauncherContext,
    state: &mut State,
) -> Screen {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut hits = HitMap::default();
    terminal
        .draw(|f| draw(f, list, PLACE, context, state, &mut hits))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = (0..buffer.area.height)
        .map(|y| {
            let text: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            (
                text.trim_end().to_string(),
                buffer[(COLUMN_X, y)].bg == SELECTED,
            )
        })
        .collect();
    Screen { rows, buffer, hits }
}

fn render(width: u16, list: &[LauncherEntry], state: &mut State) -> Vec<(String, bool)> {
    screen(width, 20, list, state).rows
}

fn row(rows: &[(String, bool)], text: &str) -> usize {
    rows.iter()
        .position(|(line, _)| line.contains(text))
        .unwrap_or_else(|| panic!("no row shows {text:?}: {rows:#?}"))
}

fn provider(id: &str, login: Login) -> ProviderLogin {
    ProviderLogin {
        id: id.into(),
        login,
    }
}

/// The mock's option block: project `mitiplay`, two providers with a login.
fn mock_context() -> LauncherContext {
    LauncherContext {
        project: Some("mitiplay".into()),
        providers: vec![
            provider("openai-codex", Login::Present),
            provider("anthropic", Login::Present),
        ],
        ..LauncherContext::default()
    }
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
fn the_header_names_corral_pi_and_the_place_and_a_rule_sits_over_the_footer() {
    for width in [185, 92, 46] {
        let shown = screen(width, 40, &with_build(), &mut State::default());
        let header = &shown.rows[0].0;
        assert_eq!(shown.col(0, "corral / pi"), 1, "{width}: {header:?}");
        assert!(header.ends_with(PLACE), "{width}: {header:?}");
        assert_eq!(
            header.chars().count(),
            usize::from(width) - 1,
            "{width}: the place ends one cell in from the right edge"
        );
        assert_eq!(shown.buffer[(1, 0)].fg, ACCENT, "{width}");
        let rule = &shown.rows[38].0;
        assert!(
            rule.chars().count() == usize::from(width) && rule.chars().all(|c| c == '─'),
            "{width}: {rule:?}"
        );
        assert!(shown.rows[39].0.contains("select"), "{width}");
        assert!(
            shown.row("What do you want to open?") < shown.row("Start"),
            "{width}"
        );
    }
    let short = screen(80, 12, &entries(), &mut State::default());
    assert!(
        !short.rows.iter().any(|(line, _)| line.contains('─')),
        "a short terminal keeps its rows for the list: {:#?}",
        short.rows
    );
}

#[test]
fn wide_rows_follow_the_grid_with_the_bar_and_the_selected_surface_on_the_selection() {
    let shown = screen(100, 20, &entries(), &mut State::default());
    let rows = &shown.rows;
    let assistant = shown.row("Assistant");
    assert!(rows[assistant].0.contains("▌ Assistant"), "{rows:#?}");
    assert!(
        rows[assistant].1,
        "the selected row is on the selected surface"
    );
    assert_eq!(shown.buffer[(COLUMN_X, assistant as u16)].fg, ACCENT);
    let pm = shown.row("PM ");
    assert!(!rows[pm].0.contains('▌') && !rows[pm].1, "{rows:#?}");
    let a31 = shown.row("A-31");
    // Bar, gap, an 11-cell name column, gap: every description starts at the same column.
    for (y, detail) in [
        (assistant, "board-aware"),
        (pm, "workspace coordinator"),
        (a31, "Cache spike · corral · 2h ago"),
    ] {
        assert_eq!(shown.col(y, detail), COLUMN_X + 14, "{detail}: {rows:#?}");
    }
    // The row's key is a key cap at the right end of the launcher column.
    let right = 100 - COLUMN_X - 1;
    assert_eq!(shown.buffer[(right - 1, assistant as u16)].symbol(), "a");
    for x in right - 2..=right {
        assert_eq!(shown.buffer[(x, assistant as u16)].bg, KEY, "cap cell {x}");
    }
    assert_eq!(shown.buffer[(right - 1, assistant as u16)].fg, ACCENT);
    assert!(shown.row("Start") < pm && pm < shown.row("Resume"));
    assert!(shown.row("Resume") < a31);
    assert_eq!(
        rows[shown.row("Resume") - 1].0,
        "",
        "a blank row above a section"
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
fn a_folded_description_wraps_at_words_under_the_name_with_the_key_cap_beside_the_name() {
    let mut list = with_build();
    list[0].detail =
        "untethered · parallel agents · nothing reaches the board until you ship".into();
    let shown = screen(46, 40, &list, &mut State::default());
    let rows = &shown.rows;
    let build = shown.row("▌ Build");
    let cap_x = 46 - COLUMN_X - 2;
    assert_eq!(shown.col(build, "b"), cap_x, "{rows:#?}");
    assert_eq!(shown.buffer[(cap_x, build as u16)].bg, KEY);
    assert_eq!(
        rows[build + 1].0.trim(),
        "untethered · parallel agents · nothing"
    );
    assert_eq!(rows[build + 2].0.trim(), "reaches the board until you ship");
    assert_eq!(shown.col(build + 1, "untethered"), COLUMN_X + 2);
    assert!(
        rows[build].1 && rows[build + 1].1 && rows[build + 2].1,
        "every line of the selected row is on the selected surface: {rows:#?}"
    );
    assert!(rows[build + 3].0.contains("Assistant"), "{rows:#?}");
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
    let context = LauncherContext::default();
    let mut state = State::default();
    state.key(&list, press(KeyCode::Left));
    state.key(&list, press(KeyCode::Down));
    assert_eq!(state.key(&list, press(KeyCode::Char('b'))), Step::Open(0));
    let launch = |id: &str, option| Launch {
        id: id.into(),
        option,
        profile: None,
    };
    assert_eq!(state.picked(&list, &context, 0), Some(launch("build", 1)));
    assert_eq!(
        state.picked(&list, &context, 1),
        Some(launch("assistant", 0))
    );
    assert_eq!(state.picked(&list, &context, list.len()), None);
}

#[test]
fn the_option_block_sits_under_the_list_and_is_live_only_while_its_row_is_selected() {
    for width in [100, 60] {
        let list = with_build();
        let mut state = State::default();
        let shown = screen(width, 24, &list, &mut state);
        let rows = &shown.rows;
        let block = shown.row("Writes to");
        assert!(
            block > shown.row("A-29"),
            "{width}: under the list: {rows:#?}"
        );
        assert!(
            rows[block - 1].0.contains("Project") && rows[block - 2].0.is_empty(),
            "{width}: Project, then where it writes, after a blank row: {rows:#?}"
        );
        assert!(
            rows[block]
                .0
                .contains("Writes to ‹ new worktree .worktrees/build-<name> ›"),
            "{width}: {rows:#?}"
        );
        assert!(!rows[block].1, "{width}: the block is not on the selection");
        let back = COLUMN_X + 10;
        assert_eq!(shown.col(block, "‹"), back, "{width}");
        assert_eq!(shown.buffer[(back, block as u16)].fg, ACCENT, "{width}");
        assert_eq!(
            shown.hit(back + 4, block),
            Some(&Hit::Key(press(KeyCode::Right))),
            "{width}: a click on the option moves to the next one"
        );
        assert_eq!(
            shown.hit(back, block),
            Some(&Hit::Key(press(KeyCode::Left))),
            "{width}: a click on ‹ moves to the previous one"
        );
        if width == 100 {
            let footer = &rows[rows.len() - 1].0;
            assert!(footer.contains("where it writes"), "{footer}");
        }
        state.key(&list, press(KeyCode::Right));
        let shown = screen(width, 24, &list, &mut state);
        shown.row("‹ here the current checkout and branch ›");
        state.key(&list, press(KeyCode::Down));
        let shown = screen(width, 24, &list, &mut state);
        let block = shown.row("‹ here the current checkout and branch ›");
        assert_eq!(
            shown.hit(COLUMN_X + 14, block),
            None,
            "{width}: Build's block is not live while Assistant is selected"
        );
        assert_ne!(shown.buffer[(back, block as u16)].fg, ACCENT, "{width}");
        assert!(
            !shown
                .rows
                .iter()
                .any(|(line, _)| line.contains("where it writes")),
            "{width}: no ←→ hint once Assistant is selected: {:#?}",
            shown.rows
        );
    }
}

#[test]
fn the_footer_names_keys_as_caps_in_order_and_keeps_fewer_on_narrow_terminals() {
    let list = with_build();
    let full = screen(185, 40, &list, &mut State::default());
    let footer = &full.rows[39].0;
    let labels = [
        "select",
        "open",
        "where it writes",
        "build",
        "assistant",
        "PM",
        "leave",
    ];
    let at: Vec<u16> = labels.iter().map(|l| full.col(39, l)).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{footer:?}");
    let up = full.col(39, "↑");
    assert_eq!(up, COLUMN_X + 1, "the first cap's glyph after its padding");
    for x in up - 1..=up + 1 {
        assert_eq!(full.buffer[(x, 39)].bg, KEY, "cap cell {x}");
    }
    assert_eq!(full.buffer[(up, 39)].fg, ACCENT);
    assert_eq!(full.hit(up, 39), Some(&Hit::Key(press(KeyCode::Up))));
    assert_eq!(
        full.hit(full.col(39, "select"), 39),
        Some(&Hit::Key(press(KeyCode::Down)))
    );
    assert_eq!(
        full.hit(full.col(39, "build"), 39),
        Some(&Hit::Key(press(KeyCode::Char('b'))))
    );
    assert_eq!(
        full.hit(full.col(39, "leave"), 39),
        Some(&Hit::Key(press(KeyCode::Esc)))
    );

    let half = screen(92, 40, &list, &mut State::default());
    let footer = &half.rows[39].0;
    assert!(
        footer.contains("build") && !footer.contains("assistant"),
        "{footer:?}"
    );
    let quarter = screen(46, 40, &list, &mut State::default());
    let footer = &quarter.rows[39].0;
    assert!(
        footer.contains("select") && footer.contains("open"),
        "{footer:?}"
    );
    assert!(!footer.contains("where it writes"), "{footer:?}");
}

#[test]
fn project_and_models_rows_frame_where_it_writes_in_the_label_column() {
    let list = with_build();
    let shown = screen_with(100, 24, &list, &mock_context(), &mut State::default());
    let rows = &shown.rows;
    let project = shown.row("Project");
    let models = shown.row("Models");
    assert_eq!(shown.row("Writes to"), project + 1, "{rows:#?}");
    assert_eq!(models, project + 2, "{rows:#?}");
    assert!(
        rows[project]
            .0
            .contains("Project   mitiplay from this directory"),
        "{rows:#?}"
    );
    assert!(
        rows[models]
            .0
            .contains("Models    openai-codex ✓   anthropic ✓ agents may use either"),
        "{rows:#?}"
    );
    assert_eq!(shown.col(project, "mitiplay"), COLUMN_X + 10);
    assert_eq!(shown.col(models, "openai-codex"), COLUMN_X + 10);
    let tick = shown.col(models, "✓");
    assert_eq!(shown.buffer[(tick, models as u16)].fg, OK);
    for y in [project, models] {
        assert!(!rows[y].1, "{y}: not on the selection");
        assert_eq!(shown.hit(COLUMN_X + 12, y), None, "{y}: no click target");
    }

    let unsure = LauncherContext {
        project: Some("mitiplay".into()),
        providers: vec![
            provider("anthropic", Login::Missing),
            provider("llama.cpp", Login::Unknown),
        ],
        ..LauncherContext::default()
    };
    let shown = screen_with(100, 24, &list, &unsure, &mut State::default());
    let models = shown.row("Models");
    assert!(
        shown.rows[models]
            .0
            .contains("anthropic no login   llama.cpp agents may use either"),
        "{:#?}",
        shown.rows
    );
    let missing = shown.col(models, "no login");
    assert_eq!(shown.buffer[(missing, models as u16)].fg, WARN);

    let one = LauncherContext {
        project: Some("mitiplay".into()),
        providers: vec![provider("openai-codex", Login::Present)],
        ..LauncherContext::default()
    };
    let shown = screen_with(100, 24, &list, &one, &mut State::default());
    let models = &shown.rows[shown.row("Models")].0;
    assert!(
        models.ends_with("openai-codex ✓"),
        "one provider has no note: {models:?}"
    );
}

#[test]
fn a_directory_of_no_project_reads_none_the_workspace_root_and_no_providers_hide_models() {
    let shown = screen(100, 24, &with_build(), &mut State::default());
    let project = shown.row("Project");
    assert!(
        shown.rows[project]
            .0
            .contains("Project   none · the workspace root"),
        "{:#?}",
        shown.rows
    );
    let label = shown.buffer[(COLUMN_X, project as u16)].fg;
    assert_eq!(
        shown.buffer[(COLUMN_X + 10, project as u16)].fg,
        label,
        "the none form is muted like the label"
    );
    assert!(
        !shown.rows.iter().any(|(line, _)| line.contains("Models")),
        "{:#?}",
        shown.rows
    );
}

#[test]
fn the_option_block_wraps_its_values_in_their_column_at_46_columns() {
    let context = LauncherContext {
        project: Some("mitiplay".into()),
        providers: vec![
            provider("openai-codex", Login::Present),
            provider("anthropic", Login::Present),
            provider("amazon-bedrock", Login::Missing),
        ],
        ..LauncherContext::default()
    };
    let shown = screen_with(46, 40, &with_build(), &context, &mut State::default());
    let rows = &shown.rows;
    let writes = shown.row("Writes to");
    assert!(
        rows[writes]
            .0
            .ends_with("Writes to ‹ new worktree .worktrees/build-"),
        "{rows:#?}"
    );
    assert_eq!(rows[writes + 1].0.trim(), "<name> ›", "{rows:#?}");
    assert_eq!(shown.col(writes + 1, "<name>"), COLUMN_X + 10);
    let models = shown.row("Models");
    assert_eq!(models, writes + 2, "{rows:#?}");
    assert!(
        rows[models].0.ends_with("openai-codex ✓   anthropic ✓"),
        "{rows:#?}"
    );
    assert_eq!(
        rows[models + 1].0.trim(),
        "amazon-bedrock no login",
        "{rows:#?}"
    );
    assert_eq!(shown.col(models + 1, "amazon-bedrock"), COLUMN_X + 10);
    assert!(
        !rows
            .iter()
            .any(|(line, _)| line.contains("agents may use either")),
        "folded rows drop the note: {rows:#?}"
    );
}

fn profile(
    name: &str,
    model: &str,
    billing: Billing,
    unavailable: Option<&str>,
) -> LauncherProfile {
    LauncherProfile {
        name: name.into(),
        model: model.into(),
        billing,
        unavailable: unavailable.map(Into::into),
    }
}

/// [`mock_context`] offering a subscription profile, a metered one and one that cannot open now,
/// opening on `initial`.
fn profile_context(initial: Option<&str>) -> LauncherContext {
    LauncherContext {
        profiles: vec![
            profile(
                "pi-sonnet",
                "anthropic/claude-sonnet-4-5",
                Billing::Subscription,
                None,
            ),
            profile(
                "pi-opus-api",
                "anthropic/claude-opus-4-5",
                Billing::Metered,
                None,
            ),
            profile(
                "pi-sol-api",
                "openai/gpt-6.2",
                Billing::Metered,
                Some("no API key"),
            ),
        ],
        profile: initial.map(Into::into),
        ..mock_context()
    }
}

#[test]
fn tab_cycles_the_profile_from_default_on_any_row_and_the_opened_row_carries_it() {
    let list = with_build();
    let context = profile_context(None);
    let mut state = State::new(&context);
    assert_eq!(state.profile(), None, "the launcher opens on default");
    assert_eq!(
        state.picked(&list, &context, 2).map(|l| l.profile),
        Some(None),
        "default returns no profile"
    );
    state.key(&list, press(KeyCode::Down));
    assert_eq!(state.key(&list, press(KeyCode::Tab)), Step::Stay);
    assert_eq!(state.profile(), Some(0));
    assert_eq!(state.selected, 1, "Tab never moves the selection");
    state.key(&list, press(KeyCode::BackTab));
    state.key(&list, press(KeyCode::BackTab));
    assert_eq!(
        state.profile(),
        Some(2),
        "Shift+Tab wraps past default to the last"
    );
    state.key(&list, press(KeyCode::Tab));
    assert_eq!(state.profile(), None, "and Tab wraps back to default");
    state.key(&list, press(KeyCode::Tab));
    state.key(&list, press(KeyCode::Tab));
    assert_eq!(
        state.option(0),
        0,
        "the profile never changes where Build writes"
    );
    assert_eq!(state.key(&list, press(KeyCode::Char('b'))), Step::Open(0));
    assert_eq!(
        state.picked(&list, &context, 0),
        Some(Launch {
            id: "build".into(),
            option: 0,
            profile: Some("pi-opus-api".into()),
        })
    );
    let explicit = State::new(&profile_context(Some("pi-opus-api")));
    assert_eq!(
        explicit.profile(),
        Some(1),
        "--profile opens on that profile"
    );
}

#[test]
fn no_row_opens_while_an_unavailable_profile_is_chosen_and_leaving_still_works() {
    let list = with_build();
    let mut state = State::new(&profile_context(Some("pi-sol-api")));
    assert_eq!(state.key(&list, press(KeyCode::Enter)), Step::Stay);
    assert_eq!(state.key(&list, press(KeyCode::Char('p'))), Step::Stay);
    assert_eq!(state.selected, 2, "the row key still selects its row");
    assert_eq!(state.click(2), Step::Stay);
    for key in [
        press(KeyCode::Esc),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        assert_eq!(state.clone().key(&list, key), Step::Leave, "{key:?}");
    }
    state.key(&list, press(KeyCode::Tab));
    assert_eq!(state.profile(), None);
    assert_eq!(state.key(&list, press(KeyCode::Enter)), Step::Open(2));
}

#[test]
fn the_profile_line_sits_over_the_block_names_billing_and_cycles_on_a_click() {
    let list = with_build();
    let context = profile_context(None);
    let mut state = State::new(&context);
    let shown = screen_with(100, 30, &list, &context, &mut state);
    let rows = &shown.rows;
    let line = shown.row("Profile");
    assert!(
        line > shown.row("A-29") && rows[line - 1].0.is_empty(),
        "{rows:#?}"
    );
    assert_eq!(shown.row("Project"), line + 1, "{rows:#?}");
    assert!(
        rows[line]
            .0
            .contains("Profile   ‹ default › each mode keeps its own profile"),
        "{rows:#?}"
    );
    let back = shown.col(line, "‹");
    assert_eq!(back, COLUMN_X + 10);
    assert_eq!(shown.buffer[(back, line as u16)].fg, ACCENT);
    assert_eq!(
        shown.hit(back, line),
        Some(&Hit::Key(press(KeyCode::BackTab)))
    );
    assert_eq!(
        shown.hit(back + 4, line),
        Some(&Hit::Key(press(KeyCode::Tab)))
    );
    let footer = rows.len() - 1;
    let cap = shown.col(footer, "tab");
    assert!(rows[footer].0.contains("tab  profile"), "{rows:#?}");
    assert_eq!(shown.hit(cap, footer), Some(&Hit::Key(press(KeyCode::Tab))));

    state.key(&list, press(KeyCode::Tab));
    let shown = screen_with(100, 30, &list, &context, &mut state);
    let text = &shown.rows[line].0;
    assert!(
        text.contains("‹ pi-sonnet › anthropic/claude-sonnet-4-5 · subscription"),
        "{text:?}"
    );
    state.key(&list, press(KeyCode::Tab));
    let shown = screen_with(100, 30, &list, &context, &mut state);
    let text = &shown.rows[line].0;
    assert!(
        text.contains("‹ pi-opus-api › anthropic/claude-opus-4-5 · API · metered"),
        "{text:?}"
    );
    let metered = shown.col(line, "API");
    assert_eq!(shown.buffer[(metered, line as u16)].fg, WARN);
    state.key(&list, press(KeyCode::Tab));
    let shown = screen_with(100, 30, &list, &context, &mut state);
    let text = &shown.rows[line].0;
    assert!(text.contains("· unavailable: no API key"), "{text:?}");
    let reason = shown.col(line, "unavailable");
    assert_eq!(shown.buffer[(reason, line as u16)].fg, WARN);

    let narrow = screen_with(46, 40, &list, &context, &mut State::new(&context));
    let line = narrow.row("Profile");
    assert_eq!(
        narrow.rows[line + 1].0.trim(),
        "own profile",
        "{:#?}",
        narrow.rows
    );
    assert_eq!(narrow.col(line + 1, "own"), COLUMN_X + 10);
}
