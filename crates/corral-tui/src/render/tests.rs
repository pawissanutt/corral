use corral_core::ArtifactKind;
use ratatui::style::Color;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::theme::{CYAN, GREEN, RED};

fn plain(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

const REPORT: &str = "# Release notes\n\nIntro with **bold**, *italic*, `code` and a [link](https://example.com/x).\n\n- first item\n  - nested item\n- second item\n\n1. one\n2. two\n\n> quoted words\n\n---\n\n| name | value |\n|------|-------|\n| alpha | a very long cell that has to wrap inside its column |\n| beta | 2 |\n\n```rust\nfn main() { println!(\"a long code line that must not wrap\"); }\n```\n";

#[test]
fn markdown_reads_as_formatted_text_not_source() {
    let lines = plain(&render(&Format::Markdown, REPORT, 100));
    assert!(
        has(&lines, "Release notes") && !has(&lines, "# Release"),
        "{lines:#?}"
    );
    assert!(
        has(
            &lines,
            "Intro with bold, italic, code and a link (https://example.com/x)."
        ),
        "{lines:#?}"
    );
    assert!(
        has(&lines, "• first item") && has(&lines, "  ◦ nested item"),
        "{lines:#?}"
    );
    assert!(has(&lines, "1. one") && has(&lines, "2. two"), "{lines:#?}");
    assert!(has(&lines, "│ quoted words"), "{lines:#?}");
    assert!(has(&lines, &"─".repeat(100)), "rule spans the width");
    assert!(!has(&lines, "**") && !has(&lines, "```"), "{lines:#?}");
}

#[test]
fn markdown_headings_carry_weight_and_color_and_links_dim_the_url() {
    let lines = render(&Format::Markdown, "# Title\n\n[text](https://e.x)\n", 40);
    let title = &lines[0].spans[0];
    assert_eq!(title.style.fg, Some(CYAN));
    assert!(
        title
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    let link = lines
        .iter()
        .find(|l| plain(std::slice::from_ref(l))[0].contains("https"))
        .unwrap();
    let url = link
        .spans
        .iter()
        .find(|s| s.content.contains("https"))
        .unwrap();
    assert_eq!(url.style.fg, Some(crate::theme::DIM));
}

#[test]
fn markdown_wraps_prose_and_table_cells_but_not_code() {
    for width in [24u16, 40, 80] {
        let lines = plain(&render(&Format::Markdown, REPORT, width));
        let table: Vec<&String> = lines
            .iter()
            .filter(|l| "┌│├└".contains(l.chars().next().unwrap_or(' ')))
            .collect();
        assert!(table.len() >= 6, "table drawn at {width}: {lines:#?}");
        for l in &table {
            assert!(
                l.width() <= usize::from(width),
                "table row fits {width}: {l:?}"
            );
        }
        let prose = lines.iter().find(|l| l.contains("Intro")).unwrap();
        assert!(prose.width() <= usize::from(width), "{prose:?}");
        assert!(
            has(
                &lines,
                "fn main() { println!(\"a long code line that must not wrap\"); }"
            ),
            "code stays on one line at {width}"
        );
    }
    let narrow = plain(&render(&Format::Markdown, REPORT, 24));
    assert!(
        has(&narrow, "wrap") && !has(&narrow, "a very long cell that has to wrap"),
        "cell wrapped: {narrow:#?}"
    );
}

#[test]
fn json_is_pretty_printed_and_highlighted() {
    let lines = render(&Format::Json, "{\"a\":[1,true,null],\"b\":\"x\"}", 80);
    let text = plain(&lines);
    assert_eq!(text[0], "{");
    assert!(text.contains(&"  \"a\": [".to_string()), "{text:#?}");
    let key = lines[1]
        .spans
        .iter()
        .find(|s| s.content == "\"a\"")
        .unwrap();
    assert_eq!(key.style.fg, Some(CYAN));
    let s = lines
        .iter()
        .flat_map(|l| &l.spans)
        .find(|s| s.content == "\"x\"")
        .unwrap();
    assert_eq!(s.style.fg, Some(GREEN));
}

#[test]
fn toml_and_yaml_highlight_keys_tables_and_comments() {
    let toml = render(&Format::Toml, "# c\n[table]\nkey = \"v\"\nn = 3\n", 80);
    assert_eq!(
        toml[0].spans.last().unwrap().style.fg,
        Some(crate::theme::DIM)
    );
    assert_eq!(toml[1].spans.last().unwrap().style.fg, Some(CYAN));
    assert!(
        toml[2]
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "key" && s.style.fg == Some(CYAN))
    );
    let yaml = render(&Format::Yaml, "name: corral\nitems:\n  - a\n", 80);
    assert!(
        yaml[0]
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "name" && s.style.fg == Some(CYAN))
    );
}

#[test]
fn diffs_color_added_removed_hunks_and_file_headers() {
    let lines = render(
        &Format::Diff,
        "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n",
        80,
    );
    let fg = |i: usize| lines[i].spans[0].style.fg;
    assert_eq!(fg(2), Some(CYAN));
    assert_eq!(fg(3), Some(RED));
    assert_eq!(fg(4), Some(GREEN));
    assert!(
        lines[0].spans[0]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
}

#[test]
fn logs_keep_ansi_colors_and_highlight_errors_and_test_results() {
    let text = "plain\n\x1b[31mred\x1b[0m tail\nwarning: careful\nerror[E0308]: mismatched\ntest result: ok. 3 passed; 0 failed\ntest result: FAILED. 1 passed; 1 failed\nprogress 10%\rprogress 100%\n";
    let lines = render(&Format::Log, text, 80);
    let t = plain(&lines);
    assert_eq!(t[1], "red tail", "escapes removed");
    assert_eq!(lines[1].spans[0].style.fg, Some(Color::Indexed(1)));
    assert_eq!(lines[2].spans[0].style.fg, Some(crate::theme::YELLOW));
    assert_eq!(lines[3].spans[0].style.fg, Some(RED));
    assert_eq!(lines[4].spans[0].style.fg, Some(GREEN));
    assert_eq!(lines[5].spans[0].style.fg, Some(RED));
    assert_eq!(t[6], "progress 100%", "carriage-return redraws collapse");
    assert_eq!(first_error_line(text), Some(3));
    assert_eq!(
        first_error_line("all good\ntest result: ok. 1 passed; 0 failed\n"),
        None
    );
}

#[test]
fn csv_renders_as_an_aligned_table() {
    let lines = plain(&render(&Format::Csv, "name,qty\nwidget,\"1,000\"\n", 40));
    assert!(
        lines[0].starts_with('┌') && has(&lines, "│ name   │ qty   │"),
        "{lines:#?}"
    );
    assert!(has(&lines, "1,000"), "{lines:#?}");
}

#[test]
fn detect_picks_formats_by_extension_then_kind() {
    assert_eq!(detect("a.md", ArtifactKind::Evidence), Format::Markdown);
    assert_eq!(detect("a.json", ArtifactKind::Report), Format::Json);
    assert_eq!(
        detect("a.rs", ArtifactKind::Evidence),
        Format::Code("rs".into())
    );
    assert_eq!(detect("gate", ArtifactKind::Log), Format::Log);
    assert_eq!(detect("notes", ArtifactKind::Summary), Format::Markdown);
    assert_eq!(detect("notes", ArtifactKind::Other), Format::Plain);
}

#[test]
fn raw_preference_is_remembered_per_kind() {
    assert!(!is_raw(ArtifactKind::Report));
    assert!(toggle_raw(ArtifactKind::Report));
    assert!(is_raw(ArtifactKind::Report) && !is_raw(ArtifactKind::Log));
    assert!(!toggle_raw(ArtifactKind::Report));
}

#[test]
fn ansi_output_round_trips_colors_and_plain_strips_them() {
    let lines = render(&Format::Diff, "+added\n", 40);
    assert!(
        to_ansi(&lines).contains("\x1b[0;38;2;"),
        "{:?}",
        to_ansi(&lines)
    );
    assert_eq!(to_plain(&lines), "+added\n");
}

#[test]
fn ansi_state_carries_across_lines_until_it_is_reset() {
    let text = "\x1b[31;1mfirst\nsecond\n\x1b[0mthird\n\x1b[44mbg\nstill bg\x1b[49m\nplain\n";
    let lines = render(&Format::Log, text, 80);
    let style = |i: usize| lines[i].spans[0].style;
    assert_eq!(style(0).fg, Some(Color::Indexed(1)));
    assert_eq!(
        style(1).fg,
        Some(Color::Indexed(1)),
        "colour persists past the newline"
    );
    assert!(
        style(1)
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD),
        "bold persists too"
    );
    assert_eq!(style(2).fg, Some(crate::theme::FG), "reset ends it");
    assert!(
        !style(2)
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    assert_eq!(style(3).bg, Some(Color::Indexed(4)));
    assert_eq!(style(4).bg, Some(Color::Indexed(4)), "background persists");
    assert_eq!(style(5).bg, None, "bg reset ends it");
    assert_eq!(plain(&lines)[1], "second", "escapes are stripped");
}

fn long_log() -> String {
    (1..=2500)
        .map(|i| format!("line {i}\n"))
        .chain(["error: final failure\n".to_string()])
        .collect()
}

#[test]
fn document_windows_find_errors_past_two_thousand_lines_and_keep_ansi_state() {
    let doc = Document::new(Format::Log, long_log());
    assert_eq!(doc.line_count(), 2501);
    assert_eq!(doc.first_error(), Some(2500));
    let window = plain(&doc.lines(false, 2499..2501));
    assert_eq!(window, ["line 2500", "error: final failure"]);

    let colored = format!("\x1b[32m{}", long_log());
    let doc = Document::new(Format::Log, colored);
    let line = &doc.lines(false, 1200..1201)[0];
    assert_eq!(
        line.spans[0].style.fg,
        Some(Color::Indexed(2)),
        "a window starts with the state its predecessors left"
    );
    assert_eq!(
        plain(&doc.lines(true, 0..1))[0],
        "line 1",
        "raw strips escapes"
    );
}

#[test]
fn multi_megabyte_logs_render_only_the_requested_window() {
    let line = "2026-01-01T00:00:00Z INFO worker tick completed without incident, queue depth 12\n";
    let text = line.repeat(40_000);
    assert!(text.len() > 3_000_000);
    let doc = Document::new(
        Format::Log,
        format!("{text}\x1b[31mpanicked at src/lib.rs\n"),
    );
    assert_eq!(doc.line_count(), 40_001);
    assert_eq!(doc.first_error(), Some(40_000));
    assert!(doc.widest() >= line.trim_end().len());
    let window = doc.lines(false, 20_000..20_040);
    assert_eq!(window.len(), 40, "a viewport-sized slice, not the document");
    assert!(!doc.needs_whole());
    let md = Document::new(Format::Markdown, "word ".repeat(300_000));
    assert!(
        !md.needs_whole(),
        "huge Markdown reads as plain lines instead of a whole layout"
    );
}

#[test]
fn failure_counts_are_whole_numeric_words_and_passing_names_are_not_errors() {
    for line in [
        "10 failed, 2 passed",
        "20 failed",
        "1 failed",
        "=== 2 failed, 10 passed in 0.5s ===",
        "build failed",
        "3 errors",
        "1 error",
        "error[E0308]: mismatched types",
        "test tests::it ... FAILED",
    ] {
        assert_eq!(first_error_line(line), Some(0), "{line}");
    }
    for line in [
        "0 failed",
        "0 failed; 12 passed",
        "0 errors",
        "test tests::rejects_error ... ok",
        "test error_handling::rejects ... ok",
        "test tests::error ... ok",
        "tests/test_x.py::test_error PASSED [ 10%]",
        "test result: ok. 3 passed; 0 failed",
    ] {
        assert_eq!(first_error_line(line), None, "{line}");
    }
    let log = "test tests::rejects_error ... ok\nerror: build failed\n";
    assert_eq!(first_error_line(log), Some(1), "points at the real failure");
    let lines = render(
        &Format::Log,
        "10 failed, 2 passed\ntest tests::rejects_error ... ok\n",
        80,
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(RED));
    assert_ne!(
        lines[1].spans[0].style.fg,
        Some(RED),
        "a passing test record is not highlighted as an error"
    );
}

#[test]
fn markdown_hard_breaks_keep_two_lines_inside_lists_and_quotes() {
    for md in ["alpha  \nbeta\n", "alpha\\\nbeta\n"] {
        assert_eq!(
            plain(&render(&Format::Markdown, md, 40)),
            ["alpha", "beta"],
            "{md:?}"
        );
    }
    assert_eq!(
        plain(&render(&Format::Markdown, "- one  \n  two\n- three\n", 40)),
        ["• one", "  two", "• three"]
    );
    assert_eq!(
        plain(&render(&Format::Markdown, "> a  \n> b\n", 40)),
        ["│ a", "│ b"]
    );
    assert_eq!(
        plain(&render(&Format::Markdown, "1. x\\\n   y\n", 40)),
        ["1. x", "   y"]
    );
    assert_eq!(
        plain(&render(&Format::Markdown, "soft\nbreak\n", 40)),
        ["soft break"],
        "a soft line break still joins"
    );
}
