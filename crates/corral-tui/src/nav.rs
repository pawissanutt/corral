//! The terminal-engine keys a newcomer needs (back to the board, leave the session), read from herdr's
//! `config.toml` and named in plain words ("Ctrl-b k", "press Ctrl-b, then k").

use toml_edit::{DocumentMut, Item};

const DEFAULT_PREFIX: &str = "ctrl+b";

/// One key binding as herdr spells it: `prefix+k` (prefix mode, then k) or a direct chord like `ctrl+alt+b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The prefix chord when the binding starts with `prefix+`.
    prefix: Option<String>,
    key: String,
}

impl Binding {
    fn parse(spec: &str, prefix: &str) -> Binding {
        match spec.strip_prefix("prefix+") {
            Some(key) => Binding {
                prefix: Some(prefix.to_string()),
                key: key.to_string(),
            },
            None => Binding {
                prefix: None,
                key: spec.to_string(),
            },
        }
    }

    /// Compact form for footers and key columns: `Ctrl-b k`, `Ctrl-Alt-b`.
    pub fn short(&self) -> String {
        match &self.prefix {
            Some(p) => format!("{} {}", chord(p), chord(&self.key)),
            None => chord(&self.key),
        }
    }

    /// Sentence form: `press Ctrl-b, then k`.
    pub fn words(&self) -> String {
        match &self.prefix {
            Some(p) => format!("press {}, then {}", chord(p), chord(&self.key)),
            None => format!("press {}", chord(&self.key)),
        }
    }
}

/// `ctrl+alt+b` → `Ctrl-Alt-b`; herdr's named punctuation becomes the character.
fn chord(spec: &str) -> String {
    spec.split('+')
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "ctrl" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            "cmd" => "Cmd".to_string(),
            "super" => "Super".to_string(),
            "minus" => "-".to_string(),
            "plus" => "+".to_string(),
            "comma" => ",".to_string(),
            "ampersand" => "&".to_string(),
            "backtick" => "`".to_string(),
            "esc" => "Esc".to_string(),
            "enter" => "Enter".to_string(),
            "tab" => "Tab".to_string(),
            "space" => "Space".to_string(),
            f if f.len() > 1
                && f.starts_with('f')
                && f[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                f.to_uppercase()
            }
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// The keys corral tells people about. `board` is None when corral's keys are not in the herdr config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavKeys {
    pub board: Option<Binding>,
    pub detach: Binding,
}

impl Default for NavKeys {
    /// What `corral up` writes into a fresh herdr config.
    fn default() -> NavKeys {
        NavKeys {
            board: Some(Binding::parse("prefix+k", DEFAULT_PREFIX)),
            detach: Binding::parse("prefix+q", DEFAULT_PREFIX),
        }
    }
}

impl NavKeys {
    /// Reads herdr's config; an unreadable or missing config means corral's keys are not set up.
    pub fn load() -> NavKeys {
        corral_herdr::config_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map_or_else(|| NavKeys::parse(""), |text| NavKeys::parse(&text))
    }

    pub fn parse(text: &str) -> NavKeys {
        let doc = text.parse::<DocumentMut>().unwrap_or_default();
        let keys = doc.get("keys");
        let field = |name: &str| keys.and_then(|k| k.get(name)).and_then(Item::as_str);
        let prefix = field("prefix").unwrap_or(DEFAULT_PREFIX);
        let board = keys
            .and_then(|k| k.get("command"))
            .and_then(|c| match c {
                Item::ArrayOfTables(tables) => tables
                    .iter()
                    .find(|t| t.get("command").and_then(Item::as_str) == Some("corral.board"))
                    .and_then(|t| t.get("key").and_then(Item::as_str).map(String::from)),
                Item::Value(v) => v.as_array().and_then(|a| {
                    a.iter().filter_map(|e| e.as_inline_table()).find_map(|t| {
                        (t.get("command").and_then(|v| v.as_str()) == Some("corral.board"))
                            .then(|| t.get("key").and_then(|v| v.as_str()).map(String::from))
                            .flatten()
                    })
                }),
                _ => None,
            })
            .filter(|key| !key.is_empty());
        NavKeys {
            board: board.map(|spec| Binding::parse(&spec, prefix)),
            detach: Binding::parse(field("detach").unwrap_or("prefix+q"), prefix),
        }
    }

    /// The back-to-board key in compact form, or how to get one.
    pub fn board_short(&self) -> String {
        self.board
            .as_ref()
            .map_or_else(|| "corral up".to_string(), Binding::short)
    }

    /// How to get back to the board from an agent pane, as a sentence fragment.
    pub fn board_words(&self) -> String {
        match &self.board {
            Some(b) => b.words(),
            None => "run `corral up` (or `corral doctor --fix` to add the key)".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_keys_corral_up_writes() {
        let nav = NavKeys::default();
        assert_eq!(nav.board_short(), "Ctrl-b k");
        assert_eq!(nav.board_words(), "press Ctrl-b, then k");
        assert_eq!(nav.detach.short(), "Ctrl-b q");
    }

    #[test]
    fn configured_prefix_and_direct_keys_are_named_plainly() {
        // Given a config with a custom prefix, a direct board chord and a custom detach.
        let nav = NavKeys::parse(
            "[keys]\nprefix = \"ctrl+a\"\ndetach = \"prefix+shift+d\"\n\n\
             [[keys.command]]\nkey = \"ctrl+alt+b\"\ntype = \"plugin_action\"\ncommand = \"corral.board\"\n",
        );
        // Then each key is spelled out the way it is pressed.
        assert_eq!(nav.board_short(), "Ctrl-Alt-b");
        assert_eq!(nav.board_words(), "press Ctrl-Alt-b");
        assert_eq!(nav.detach.short(), "Ctrl-a Shift-d");
        assert_eq!(nav.detach.words(), "press Ctrl-a, then Shift-d");
    }

    #[test]
    fn prefix_bindings_follow_the_configured_prefix_and_inline_arrays() {
        let nav = NavKeys::parse(
            "[keys]\nprefix = \"f12\"\ncommand = [{ key = \"prefix+minus\", command = \"corral.board\" }]\n",
        );
        assert_eq!(nav.board_short(), "F12 -");
    }

    #[test]
    fn missing_corral_keys_point_to_corral_up() {
        let nav = NavKeys::parse("[ui]\nonboarding = false\n");
        assert_eq!(nav.board, None);
        assert_eq!(nav.board_short(), "corral up");
        assert!(nav.board_words().contains("corral doctor --fix"));
        assert_eq!(nav.detach.short(), "Ctrl-b q");
        assert_eq!(NavKeys::parse("not toml ["), NavKeys::parse(""));
    }
}
