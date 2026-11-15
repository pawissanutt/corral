use super::*;

fn parsed(text: &str) -> TerminalSnapshot {
    let snapshot = snapshot(text).unwrap();
    assert_eq!(snapshot.lines.len(), snapshot.rows.len());
    for (line, row) in snapshot.lines.iter().zip(&snapshot.rows) {
        assert_eq!(
            *line,
            row.iter().map(|run| run.text.as_str()).collect::<String>()
        );
        assert!(row.iter().all(|run| !run.text.is_empty()));
    }
    snapshot
}

#[test]
fn extended_colours_accept_both_separators_and_do_not_desynchronise_sgr() {
    let screen = parsed(concat!(
        "\x1b[38;2;10;20;30;48;5;200ma",
        "\x1b[38:5:42;48:2:40:50:60mb",
        "\x1b[38:2::70:80:90;48:2::1:2:3mc",
        "\x1b[38;5;255;48;2;4;5;6md",
        "\x1b[38;2;999;0;1;777;3me",
        "\x1b[999:4;38;999;9mf",
    ));
    let row = &screen.rows[0];
    assert_eq!(row.len(), 6);
    assert_eq!(row[0].fg, Some(TerminalColor::Rgb([10, 20, 30])));
    assert_eq!(row[0].bg, Some(TerminalColor::Indexed(200)));
    assert_eq!(row[1].fg, Some(TerminalColor::Indexed(42)));
    assert_eq!(row[1].bg, Some(TerminalColor::Rgb([40, 50, 60])));
    assert_eq!(row[2].fg, Some(TerminalColor::Rgb([70, 80, 90])));
    assert_eq!(row[2].bg, Some(TerminalColor::Rgb([1, 2, 3])));
    assert_eq!(row[3].fg, Some(TerminalColor::Indexed(255)));
    assert_eq!(row[3].bg, Some(TerminalColor::Rgb([4, 5, 6])));
    assert_eq!(row[4].fg, row[3].fg);
    assert!(row[4].italic);
    assert!(!row[4].bold);
    assert!(!row[4].underline);
    assert!(row[5].strike);
}

#[test]
fn basic_and_bright_colours_reset_to_defaults() {
    let screen = parsed("\x1b[30;47ma\x1b[97;100mb\x1b[39;49mc\x1b[31;44md\x1b[me");
    let row = &screen.rows[0];
    assert_eq!(row[0].fg, Some(TerminalColor::Indexed(0)));
    assert_eq!(row[0].bg, Some(TerminalColor::Indexed(7)));
    assert_eq!(row[1].fg, Some(TerminalColor::Indexed(15)));
    assert_eq!(row[1].bg, Some(TerminalColor::Indexed(8)));
    for run in [&row[2], &row[4]] {
        assert_eq!(run.fg, None);
        assert_eq!(run.bg, None);
    }
    assert_eq!(row[3].fg, Some(TerminalColor::Indexed(1)));
    assert_eq!(row[3].bg, Some(TerminalColor::Indexed(4)));
}

#[test]
fn attributes_clear_independently_and_equal_styles_merge_across_sequences() {
    let screen = parsed(concat!(
        "\x1b[1;2;3;4;7;9ma\x1b[1mb",
        "\x1b[22;23;24;27;29mc",
        "\x1b[21md\x1b[4:0me\x1b[4:3mf\x1b[0mg",
    ));
    let row = &screen.rows[0];
    assert_eq!(row.len(), 6);
    assert_eq!(row[0].text, "ab");
    assert!(row[0].bold && row[0].dim && row[0].italic && row[0].underline);
    assert!(row[0].inverse && row[0].strike);
    assert_eq!(
        row[1],
        TerminalRun {
            text: "c".into(),
            ..TerminalRun::default()
        }
    );
    assert!(row[2].underline);
    assert!(!row[3].underline);
    assert!(row[4].underline);
    assert_eq!(
        row[5],
        TerminalRun {
            text: "g".into(),
            ..TerminalRun::default()
        }
    );
}

#[test]
fn terminal_controls_and_hyperlinks_are_dropped_without_eating_visible_text() {
    let screen = parsed(concat!(
        "A\x1b[2KB\x1b]8;;https://example.test\x07link",
        "\x1b]8;;\x1b\\C\x1b(BD\t\r\x00",
        "\x1b]title\x1b\\E\x1bPignored\x1b\\F\x1b",
    ));
    assert_eq!(screen.lines, ["ABlinkCDEF"]);
    assert_eq!(screen.rows[0].len(), 1);
}

#[test]
fn styles_cross_newlines_and_only_invisible_trailing_whitespace_is_trimmed() {
    let screen = parsed(concat!(
        "\x1b[31mfirst \nsecond\x1b[0m   \n",
        "   \n\x1b[44m  \x1b[49m   \n",
        "\x1b[7m \x1b[27m \n\x1b[4m \x1b[24m \n",
        "\x1b[9m \x1b[29m \n   \n\n",
    ));
    assert_eq!(screen.lines, ["first", "second", "", "  ", " ", " ", " "]);
    assert_eq!(screen.rows[0][0].fg, Some(TerminalColor::Indexed(1)));
    assert_eq!(screen.rows[1][0].fg, Some(TerminalColor::Indexed(1)));
    assert!(screen.rows[2].is_empty());
    assert_eq!(screen.rows[3][0].bg, Some(TerminalColor::Indexed(4)));
    assert!(screen.rows[4][0].inverse);
    assert!(screen.rows[5][0].underline);
    assert!(screen.rows[6][0].strike);
}

#[test]
fn screen_bounds_keep_last_rows_and_truncate_on_character_boundaries() {
    let screen = parsed(
        &(0..220)
            .map(|n| format!("\x1b[32m{n}\n"))
            .collect::<String>(),
    );
    assert_eq!(screen.lines.len(), 200);
    assert_eq!(screen.lines.first().unwrap(), "20");
    assert_eq!(screen.lines.last().unwrap(), "219");
    let screen = parsed(&format!("{}éz", "x".repeat(2047)));
    assert_eq!(screen.lines[0], "x".repeat(2047));
    let screen = snapshot(
        &(0..30)
            .map(|n| format!("{n:02} {}\n", "\"\\".repeat(1022)))
            .collect::<String>(),
    )
    .unwrap();
    assert!(paging::json_len(&screen.lines).unwrap() <= TEXT_BUDGET);
    assert!(screen.lines.len() < 30);
    assert!(screen.lines.last().unwrap().starts_with("29 "));
}

#[test]
fn over_budget_styles_fall_back_without_changing_plain_lines() {
    let text = (0..2)
        .map(|_| {
            let mut row = (0..2048)
                .map(|n| format!("\x1b[38;5;{}mx", n % 2))
                .collect::<String>();
            row.push('\n');
            row
        })
        .collect::<String>();
    let screen = snapshot(&text).unwrap();
    assert_eq!(screen.lines, ["x".repeat(2048), "x".repeat(2048)]);
    assert!(screen.rows.is_empty());
}
