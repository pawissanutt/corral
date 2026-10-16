//! `corral artifact show` and the importance-ordered artifact lines of `corral show` / `corral review`.

use std::io::{BufRead, BufReader, IsTerminal, Write};

use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactTier};
use corral_ops::{Ctx, Preview};
use corral_tui::render;

use crate::artifact_cmd::LineRange;

/// Layout width: `COLUMNS` when set to a positive number (an explicit override), else the terminal's own size
/// when stdout is a terminal, else a fixed 100 columns for pipes.
fn render_width(tty: bool) -> u16 {
    pick_width(
        std::env::var("COLUMNS").ok().as_deref(),
        tty.then(corral_tui::term::width).flatten(),
    )
}

pub(crate) fn pick_width(columns: Option<&str>, terminal: Option<u16>) -> u16 {
    columns
        .and_then(|c| c.trim().parse::<u16>().ok())
        .filter(|w| *w > 0)
        .or(terminal)
        .unwrap_or(100)
}

/// Prints an artifact for reading: the complete decoded document rendered like the TUI preview (ANSI colours
/// on a terminal, plain text when piped), or the decoded stored bytes with `raw`. Compressed snapshots are
/// always decoded through the storage reader; purged ones print their stub.
/// A line range selects exact source lines, without rendering or buffering the document.
pub(crate) fn show(ctx: &Ctx, reference: &str, raw: bool, lines: Option<LineRange>) -> Result<()> {
    let a = corral_ops::resolve_artifact(ctx, reference)?;
    if let Some(lines) = lines {
        return copy_lines(
            &mut BufReader::new(corral_ops::artifact_reader(ctx, &a)?),
            &mut std::io::stdout().lock(),
            lines,
        );
    }
    if raw {
        return std::io::copy(
            &mut corral_ops::artifact_reader(ctx, &a)?,
            &mut std::io::stdout().lock(),
        )
        .map(|_| ())
        .context("writing artifact content");
    }
    match corral_ops::artifact_document(ctx, &a)? {
        Preview::Text(text) => {
            let width = render_width(std::io::stdout().is_terminal());
            let name = std::path::Path::new(&a.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let lines = render::render(&render::detect(&name, a.kind), &text, width);
            if std::io::stdout().is_terminal() {
                print!("{}", render::to_ansi(&lines));
            } else {
                print!("{}", render::to_plain(&lines));
            }
        }
        Preview::Image { path, note } | Preview::External { path, note } => {
            println!("{note}\n{path}");
        }
    }
    Ok(())
}

/// Streams buffer slices rather than collecting lines: even one enormous line
/// uses only the caller's fixed reader buffer. No bytes past the end are requested.
fn copy_lines(reader: &mut impl BufRead, out: &mut impl Write, range: LineRange) -> Result<()> {
    let mut line = 1;
    loop {
        let buf = reader.fill_buf().context("reading artifact lines")?;
        if buf.is_empty() {
            return Ok(());
        }
        let newline = buf.iter().position(|b| *b == b'\n');
        let count = newline.map_or(buf.len(), |at| at + 1);
        if line >= range.start {
            out.write_all(&buf[..count])
                .context("writing artifact lines")?;
        }
        reader.consume(count);
        if newline.is_some() {
            if line == range.end {
                return Ok(());
            }
            // line < range.end, so this cannot overflow even for a u64::MAX endpoint.
            line += 1;
        }
    }
}

/// Artifact lines for `corral show` / `corral review`: keep and normal items in importance order, then the
/// transient ones under a `raw (N)` fold line.
pub(crate) fn print_grouped(arts: &[Artifact], with_path: bool) {
    let mut ordered = arts.to_vec();
    corral_ops::order_artifacts(&mut ordered);
    let line = |a: &Artifact| {
        let purged = if a.purged_at.is_some() {
            " [purged]"
        } else {
            ""
        };
        if with_path {
            format!(
                "artifact {} [{}] {} -> {}{purged}",
                a.run,
                a.kind.as_str(),
                a.title,
                a.path
            )
        } else {
            format!(
                "artifact {} [{}] {}{purged}",
                a.run,
                a.kind.as_str(),
                a.title
            )
        }
    };
    let (raw, shown): (Vec<_>, Vec<_>) = ordered
        .iter()
        .partition(|a| a.tier() == ArtifactTier::Transient);
    for a in shown {
        println!("{}", line(a));
    }
    if !raw.is_empty() {
        println!("raw ({}): corral artifact list --tier transient", raw.len());
        for a in raw {
            println!("  {}", line(a));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor, Read};

    use super::{LineRange, copy_lines, pick_width};

    #[test]
    fn columns_overrides_then_the_terminal_then_a_default() {
        assert_eq!(pick_width(Some("72"), Some(80)), 72);
        assert_eq!(pick_width(None, Some(80)), 80);
        assert_eq!(pick_width(Some("0"), Some(80)), 80);
        assert_eq!(pick_width(Some("wide"), None), 100);
        assert_eq!(pick_width(None, None), 100);
    }

    #[test]
    fn range_preserves_source_bytes_across_small_buffers() {
        let source = b"first\r\n\r\nthird \xe4\xb8\x89\nlast\xff";
        for (start, end, expected) in [
            (1, 1, &b"first\r\n"[..]),
            (2, 3, &b"\r\nthird \xe4\xb8\x89\n"[..]),
            (4, 8, &b"last\xff"[..]),
            (5, 8, &b""[..]),
        ] {
            let mut reader = BufReader::with_capacity(2, Cursor::new(source));
            let mut out = Vec::new();
            copy_lines(&mut reader, &mut out, LineRange { start, end }).unwrap();
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn range_skips_a_huge_line_without_collecting_it() {
        let source = std::io::repeat(b'x')
            .take(8 * 1024 * 1024)
            .chain(Cursor::new(b"\ntarget\nremaining\n"));
        let mut reader = BufReader::with_capacity(31, source);
        let mut out = Vec::new();
        copy_lines(&mut reader, &mut out, LineRange { start: 2, end: 2 }).unwrap();
        assert_eq!(out, b"target\n");
    }

    #[test]
    fn range_does_not_read_beyond_its_last_newline() {
        struct UnreadTail;
        impl Read for UnreadTail {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("read beyond requested range"))
            }
        }
        let source = Cursor::new(b"one\n").chain(UnreadTail);
        let mut reader = BufReader::with_capacity(4, source);
        let mut out = Vec::new();
        copy_lines(&mut reader, &mut out, LineRange { start: 1, end: 1 }).unwrap();
        assert_eq!(out, b"one\n");
    }
}
