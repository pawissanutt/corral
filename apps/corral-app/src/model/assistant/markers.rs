//! The inline markers a composer draft carries for what a paste put aside: `[Image #n]` for the
//! nth staged image of the message (corral-pi names it "attached image n"), and
//! `[paste #k +n lines]` / `[paste #k n chars]` for long pasted text, expanded on send.

pub const PASTE_LINES: usize = 10;
pub const PASTE_CHARS: usize = 1000;

pub fn image_marker(n: usize) -> String {
    format!("[Image #{n}]")
}

pub fn is_long_paste(text: &str) -> bool {
    text.lines().count() > PASTE_LINES || text.chars().count() > PASTE_CHARS
}

pub fn paste_marker(k: usize, text: &str) -> String {
    let lines = text.lines().count();
    if lines > PASTE_LINES {
        format!("[paste #{k} +{lines} lines]")
    } else {
        format!("[paste #{k} {} chars]", text.chars().count())
    }
}

fn digits(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_digit).count()
}

fn replace_markers(
    mut text: &str,
    prefix: &str,
    mut replace: impl FnMut(usize, &str, &mut String, &mut &str) -> bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    while let Some((before, marker)) = text.split_once(prefix) {
        out.push_str(before);
        let (number, rest) = marker.split_at(digits(marker));
        if let (Ok(n), Some(end)) = (number.parse(), rest.find(['[', ']']))
            && rest.as_bytes()[end] == b']'
        {
            let suffix = &rest[..end];
            let mut tail = &rest[end + 1..];
            if replace(n, suffix, &mut out, &mut tail) {
                text = tail;
                continue;
            }
        }
        out.push_str(prefix);
        text = marker;
    }
    out.push_str(text);
    out
}

fn counted_suffix(text: &str, end: &str) -> bool {
    let n = digits(text);
    n > 0 && &text[n..] == end
}

pub fn expand_pastes(text: &str, pastes: &[String]) -> String {
    replace_markers(text, "[paste #", |n, suffix, out, _| {
        if !(suffix.is_empty()
            || suffix
                .strip_prefix(" +")
                .is_some_and(|s| counted_suffix(s, " lines"))
            || suffix
                .strip_prefix(' ')
                .is_some_and(|s| counted_suffix(s, " chars")))
        {
            return false;
        }
        let Some(paste) = n.checked_sub(1).and_then(|i| pastes.get(i)) else {
            return false;
        };
        out.push_str(paste);
        true
    })
}

pub fn drop_image_marker(text: &str, removed: usize) -> String {
    replace_markers(text, "[Image #", |n, suffix, out, tail| {
        if !suffix.is_empty() || n < removed {
            return false;
        }
        if n > removed {
            out.push_str(&image_marker(n - 1));
        } else if out.ends_with(' ') {
            out.pop();
        } else if let Some(after) = tail.strip_prefix(' ') {
            *tail = after;
        }
        true
    })
}

/// `text` without the `[Image #n]` markers that name one of its `backed` images, `n` from 1 to
/// `backed` (and one space beside each): a draft saved on the device keeps no such marker,
/// because the staged images live in memory only and a restored draft's marker would name
/// whatever image is pasted next. Any other `[Image #n]` is the owner's own text and stays.
pub fn strip_image_markers(text: &str, backed: usize) -> String {
    replace_markers(text, "[Image #", |n, suffix, out, tail| {
        if !suffix.is_empty() || n == 0 || n > backed {
            return false;
        }
        if out.ends_with(' ') {
            out.pop();
        } else if let Some(after) = tail.strip_prefix(' ') {
            *tail = after;
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_draft_keeps_no_image_marker_a_restart_cannot_back() {
        assert_eq!(
            strip_image_markers("[Image #1] look at [Image #2] here [Image #3]", 2),
            "look at here [Image #3]"
        );
        // Text that only mentions the syntax, with no image staged, is kept whole.
        assert_eq!(
            strip_image_markers("write [Image #1] for an image", 0),
            "write [Image #1] for an image"
        );
    }

    #[test]
    fn a_folded_paste_sends_exactly_what_was_pasted() {
        let pasted = "  copied line\r\n".repeat(30);
        assert!(is_long_paste(&pasted));
        let draft = format!("Before {} after", paste_marker(1, &pasted));
        assert_eq!(
            expand_pastes(&draft, std::slice::from_ref(&pasted)),
            format!("Before {pasted} after")
        );
    }

    #[test]
    fn removing_an_image_renumbers_the_markers_after_it() {
        assert_eq!(
            drop_image_marker("a [Image #1] b [Image #2] c [Image #3]", 2),
            "a [Image #1] b c [Image #2]"
        );
    }
}
