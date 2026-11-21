//! Pairing links are transient drafts, never implicit permission to contact a daemon.

fn fragment_code(fragment: &str) -> Option<&str> {
    fragment.strip_prefix(corral_protocol::device::PAIR_LINK_FRAGMENT)
}

#[cfg(windows)]
pub(super) fn from_input(text: &str) -> Result<corral_protocol::device::PairCode, String> {
    use corral_protocol::device::PairCode;

    let text = text.trim();
    if text.starts_with("http://") || text.starts_with("https://") {
        let link = url::Url::parse(text).map_err(|_| "the Corral pairing link is invalid")?;
        let code = link
            .fragment()
            .and_then(fragment_code)
            .ok_or("the URL does not contain a Corral pairing code")?;
        PairCode::decode(code)
    } else {
        PairCode::decode(text)
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn take_fragment(
    hash: &str,
    clear: impl FnOnce() -> Result<(), &'static str>,
) -> Result<Option<String>, &'static str> {
    let Some(code) = hash.strip_prefix('#').and_then(fragment_code) else {
        return Ok(None);
    };
    clear()?;
    Ok(Some(code.to_owned()))
}

#[cfg(target_arch = "wasm32")]
pub(super) fn read() -> Result<Option<String>, &'static str> {
    const REFUSED: &str = "The pairing link could not be removed from the address bar. Remove the fragment (everything from # onward), reload, then paste the pairing code to continue.";
    let window = web_sys::window().ok_or(REFUSED)?;
    let location = window.location();
    let hash = location.hash().map_err(|_| REFUSED)?;
    take_fragment(&hash, || {
        let history = window.history().map_err(|_| REFUSED)?;
        let state = history.state().map_err(|_| REFUSED)?;
        let href = location.href().map_err(|_| REFUSED)?;
        let clean = href.split_once('#').map_or(href.as_str(), |(url, _)| url);
        history
            .replace_state_with_url(&state, "", Some(clean))
            .map_err(|_| REFUSED)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn only_the_approved_fragment_is_consumed_and_only_after_clearing() {
        for hash in [
            "",
            "#other=keep",
            "#x=1&corral-pair=code",
            "#corral-pairing=code",
            "corral-pair=code",
        ] {
            assert_eq!(
                take_fragment(hash, || panic!("unrelated fragment must stay")),
                Ok(None)
            );
        }
        let cleared = Cell::new(false);
        let code = take_fragment("#corral-pair=corral-pair-1.fixture", || {
            cleared.set(true);
            Ok(())
        })
        .unwrap();
        assert!(cleared.get());
        assert_eq!(code.as_deref(), Some("corral-pair-1.fixture"));
        assert_eq!(
            take_fragment("#corral-pair=secret", || Err("cannot clear")),
            Err("cannot clear"),
        );
    }
}
