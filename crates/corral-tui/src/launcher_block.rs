//! The launcher's option block, what a build session would open with (its project, where it writes,
//! which model providers its agents may use), and the word wrapping it shares with folded rows.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{ACCENT, LauncherEntry, MUTED, OK, Part, State, TEXT, WARN, fg, padded};
use crate::overlays::truncate;

/// The label column of the option block.
pub(super) const LABEL_COL: usize = 10;
const PROJECT_LABEL: &str = "Project";
/// The label of where the row with options writes.
const WRITES_LABEL: &str = "Writes to";
const MODELS_LABEL: &str = "Models";
/// Between two providers on the Models row.
const PROVIDER_GAP: &str = "   ";

/// What the launcher shows about the build session it would open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherContext {
    /// The project the starting directory resolves to; `None` in a directory of no project.
    pub project: Option<String>,
    /// The model providers the session's agents can use, in the order shown; none hides the row.
    pub providers: Vec<ProviderLogin>,
}

/// A model provider with what is known of its login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLogin {
    /// The provider id (`openai-codex`, `anthropic`).
    pub id: String,
    pub login: Login,
}

/// What can be told, without starting the host, of a provider's corral-owned login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login {
    /// A usable login exists: `✓` in ok.
    Present,
    /// There is none, or it cannot be used: `no login` in warn.
    Missing,
    /// It cannot be told: the bare name.
    Unknown,
}

/// A run of text in one style.
pub(super) type Piece = (String, Style);

fn width_of(pieces: &[Piece]) -> usize {
    pieces.iter().map(|(text, _)| text.width()).sum()
}

/// `pieces` without the spaces they end in.
fn trim_end(mut pieces: Vec<Piece>) -> Vec<Piece> {
    while let Some((text, _)) = pieces.last_mut() {
        let kept = text.trim_end().len();
        text.truncate(kept);
        if !text.is_empty() {
            break;
        }
        pieces.pop();
    }
    pieces
}

/// `pieces` cut at display width `width`; the head is never empty, so a cut always makes progress.
fn split(pieces: Vec<Piece>, width: usize) -> (Vec<Piece>, Vec<Piece>) {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut used = 0;
    for (text, style) in pieces {
        if !tail.is_empty() {
            tail.push((text, style));
            continue;
        }
        let cut = text.char_indices().find_map(|(i, c)| {
            let w = c.width().unwrap_or(0);
            let over = used + w > width && (used > 0 || i > 0);
            used += w;
            over.then_some(i)
        });
        match cut {
            Some(i) => {
                if i > 0 {
                    head.push((text[..i].to_string(), style));
                }
                tail.push((text[i..].to_string(), style));
            }
            None => head.push((text, style)),
        }
    }
    (head, tail)
}

/// `pieces` in lines at most `width` wide, broken after spaces and hyphens as a browser breaks
/// normal text: the spaces at a break are dropped, a word runs on across pieces, and a word longer
/// than a line is cut.
pub(super) fn wrap(pieces: &[Piece], width: usize) -> Vec<Vec<Piece>> {
    let width = width.max(1);
    let mut words: Vec<Vec<Piece>> = Vec::new();
    let mut word: Vec<Piece> = Vec::new();
    for (text, style) in pieces {
        let mut run = String::new();
        for c in text.chars() {
            run.push(c);
            if c == ' ' || c == '-' {
                word.push((std::mem::take(&mut run), *style));
                words.push(std::mem::take(&mut word));
            }
        }
        if !run.is_empty() {
            word.push((run, *style));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut lines = Vec::new();
    let mut line: Vec<Piece> = Vec::new();
    let mut used = 0;
    for word in words {
        let w = width_of(&word);
        let blank = word.iter().all(|(text, _)| text.trim().is_empty());
        let trailing = usize::from(word.last().is_some_and(|(text, _)| text.ends_with(' ')));
        if !line.is_empty() && (blank || used + w - trailing <= width) {
            used += w;
            line.extend(word);
            continue;
        }
        if blank {
            // A line never starts with the spaces of a break.
            continue;
        }
        if !line.is_empty() {
            lines.push(trim_end(std::mem::take(&mut line)));
        }
        line = word;
        while width_of(&trim_end(line.clone())) > width {
            let (head, tail) = split(line, width);
            lines.push(head);
            line = tail;
        }
        used = width_of(&line);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(trim_end(line));
    }
    lines
}

/// Styled pieces as spans.
pub(super) fn spans(pieces: Vec<Piece>) -> Vec<Span<'static>> {
    pieces
        .into_iter()
        .map(|(text, style)| Span::styled(text, style))
        .collect()
}

/// `label` in the label column, then `value` wrapped in the `cols` after it; `part` says what line
/// `n` of the value is to the mouse.
fn labelled(
    out: &mut Vec<(Line<'static>, Part)>,
    label: &str,
    value: &[Piece],
    cols: usize,
    part: impl Fn(usize) -> Part,
) {
    for (n, pieces) in wrap(value, cols).into_iter().enumerate() {
        let label = if n == 0 { label } else { "" };
        let mut line = vec![Span::styled(
            padded(truncate(label, LABEL_COL), LABEL_COL),
            fg(MUTED),
        )];
        line.extend(spans(pieces));
        out.push((Line::from(line), part(n)));
    }
}

/// The option block under the list, in the mock's 10-cell label column: `Project` (the project the
/// directory resolves to, or `none · the workspace root`), `Writes to ‹ option ›` (where the row
/// with options writes) and `Models` (the providers, each with what is known of its login, then
/// `agents may use either` for two or more unless the rows are folded). Each value wraps in its
/// column. The block follows the selected row when that row has options, otherwise the first such
/// row; only the selected row's `Writes to` is live, with accent chevrons and click targets, and
/// the other's chevrons are muted. Without a row with options there is no block.
pub(super) fn option_block(
    entries: &[LauncherEntry],
    state: &State,
    context: &LauncherContext,
    width: usize,
    folded: bool,
) -> Vec<(Line<'static>, Part)> {
    let owner = if entries
        .get(state.selected)
        .is_some_and(|e| !e.options.is_empty())
    {
        Some(state.selected)
    } else {
        entries.iter().position(|e| !e.options.is_empty())
    };
    let Some((owner, option)) = owner.and_then(|owner| {
        let option = entries.get(owner)?.options.get(state.option(owner))?;
        Some((owner, option))
    }) else {
        return Vec::new();
    };
    let cols = width.saturating_sub(LABEL_COL);
    let mut out = Vec::new();
    let project = match &context.project {
        Some(name) => vec![
            (name.clone(), fg(TEXT)),
            (" from this directory".to_string(), fg(MUTED)),
        ],
        None => vec![("none · the workspace root".to_string(), fg(MUTED))],
    };
    labelled(&mut out, PROJECT_LABEL, &project, cols, |_| Part::Gap);
    let live = owner == state.selected;
    let chevron = fg(if live { ACCENT } else { MUTED });
    let option = option.split_whitespace().collect::<Vec<_>>().join(" ");
    let writes = vec![
        ("‹".to_string(), chevron),
        (format!(" {option} "), fg(TEXT)),
        ("›".to_string(), chevron),
    ];
    labelled(&mut out, WRITES_LABEL, &writes, cols, |n| match (live, n) {
        (false, _) => Part::Gap,
        (true, 0) => Part::Option(Some(LABEL_COL)),
        (true, _) => Part::Option(None),
    });
    if context.providers.is_empty() {
        return out;
    }
    let mut models = Vec::new();
    for (i, provider) in context.providers.iter().enumerate() {
        if i > 0 {
            models.push((PROVIDER_GAP.to_string(), fg(TEXT)));
        }
        models.push((provider.id.clone(), fg(TEXT)));
        match provider.login {
            Login::Present => models.push((" ✓".to_string(), fg(OK))),
            Login::Missing => models.push((" no login".to_string(), fg(WARN))),
            Login::Unknown => {}
        }
    }
    if context.providers.len() > 1 && !folded {
        models.push((" agents may use either".to_string(), fg(MUTED)));
    }
    labelled(&mut out, MODELS_LABEL, &models, cols, |_| Part::Gap);
    out
}
