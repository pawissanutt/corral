use corral_protocol::detail::{TerminalColor, TerminalRun};

use super::{Defaults, Ink, indexed, rows};

fn run(text: &str) -> TerminalRun {
    TerminalRun {
        text: text.into(),
        ..TerminalRun::default()
    }
}

const DEFAULTS: Defaults = Defaults {
    fore: [200; 3],
    back: [20; 3],
};

#[test]
fn indexed_colours_change_table_at_the_xterm_boundaries() {
    assert_eq!(indexed(15), [0xF2, 0xF2, 0xF2]);
    assert_eq!(indexed(16), [0, 0, 0]);
    assert_eq!(indexed(17), [0, 0, 95]);
    assert_eq!(indexed(22), [0, 95, 0]);
    assert_eq!(indexed(52), [95, 0, 0]);
    assert_eq!(indexed(231), [255, 255, 255]);
    assert_eq!(indexed(232), [8, 8, 8]);
    assert_eq!(indexed(255), [238, 238, 238]);
}

#[test]
fn inverse_swaps_the_default_colours_as_well_as_set_ones() {
    let screen = rows(&[vec![
        TerminalRun {
            inverse: true,
            ..run("x")
        },
        TerminalRun {
            fg: Some(TerminalColor::Indexed(1)),
            inverse: true,
            ..run("y")
        },
    ]]);
    let [plain, red] = screen[0].as_slice() else {
        panic!("two bands: {screen:?}");
    };
    assert_eq!(DEFAULTS.fill(plain.back), Some([200; 3]));
    assert_eq!(
        DEFAULTS.text(plain.pieces[0].runs[0].look, plain.back),
        [20; 3]
    );
    assert_eq!(DEFAULTS.fill(red.back), Some(indexed(1)));
    assert_eq!(DEFAULTS.text(red.pieces[0].runs[0].look, red.back), [20; 3]);
}

#[test]
fn dim_fades_toward_the_runs_own_background() {
    let screen = rows(&[vec![TerminalRun {
        fg: Some(TerminalColor::Rgb([255, 255, 255])),
        bg: Some(TerminalColor::Rgb([0, 0, 255])),
        dim: true,
        ..run("d")
    }]]);
    let band = &screen[0][0];
    assert_eq!(
        DEFAULTS.text(band.pieces[0].runs[0].look, band.back),
        [153, 153, 255]
    );
}

#[test]
fn only_a_background_change_starts_a_new_band() {
    let blue = Some(TerminalColor::Rgb([0, 0, 255]));
    let screen = rows(&[vec![
        TerminalRun {
            bg: blue,
            ..run("ab")
        },
        TerminalRun {
            bg: blue,
            bold: true,
            ..run("c")
        },
        run("de"),
    ]]);
    let row = &screen[0];
    assert_eq!(row.len(), 2);
    assert_eq!(row[0].cells, 3);
    assert_eq!(row[0].pieces.len(), 1);
    assert_eq!(row[0].pieces[0].runs.len(), 2);
    assert_eq!((row[1].back, row[1].cells), (Ink::Back, 2));
}

#[test]
fn characters_off_the_face_or_wide_get_pieces_of_their_own_cells() {
    let screen = rows(&[vec![run("│ ✔\u{FE0F} e\u{301} 📝│")]]);
    let band = &screen[0][0];
    let pieces: Vec<(String, usize, bool)> = band
        .pieces
        .iter()
        .map(|piece| {
            let text = piece.runs.iter().map(|run| run.text.as_str()).collect();
            (text, piece.cells, piece.boxed)
        })
        .collect();
    assert_eq!(
        pieces,
        vec![
            ("│ ".to_owned(), 2, false),
            ("✔\u{FE0F}".to_owned(), 1, true),
            (" e\u{301} ".to_owned(), 3, false),
            ("📝".to_owned(), 2, true),
            ("│".to_owned(), 1, false),
        ]
    );
    assert_eq!(band.cells, 9);
}
