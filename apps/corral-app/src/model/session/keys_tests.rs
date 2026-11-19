use super::{KeyAction, WEB_RESERVED, bound};
use crate::model::nav::Form;

/// A session key that shadows one the browser keeps (F5 reload, Ctrl+L the address bar, Ctrl+T
/// and Ctrl+W tabs, Ctrl+P print…) either never reaches the page or takes the browser's own
/// command from the owner. The web build must bind none of them, whatever a part declares,
/// while the remapped keys that replace them are still bound.
#[test]
fn no_session_key_shadows_a_browser_reserved_key_on_the_web() {
    for form in [Form::Desktop, Form::Terminal] {
        let web = bound(form, true);
        for binding in &web {
            let key = binding.key.replace("mod+", "ctrl+");
            assert!(
                !WEB_RESERVED.contains(&key.as_str()),
                "{form:?}: {} shadows a browser key",
                binding.key
            );
        }
        for action in [
            KeyAction::Thinking,
            KeyAction::ExpandAll,
            KeyAction::Jump,
            KeyAction::Model,
            KeyAction::Effort,
        ] {
            assert!(
                web.iter().any(|binding| binding.action == action),
                "{form:?}: {action:?} lost its web binding"
            );
        }
    }
    // The native build keeps the terminal UI's F5 for the Task tab.
    assert!(
        bound(Form::Desktop, false)
            .iter()
            .any(|binding| binding.key == "f5")
    );
    assert!(bound(Form::Phone, true).is_empty());
}
