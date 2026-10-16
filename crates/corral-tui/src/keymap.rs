//! Translate physical board keys once; clicks retain their canonical action.

use corral_core::UiCfg;

pub(crate) fn label(label: &str, ui: &UiCfg) -> String {
    if let [key] = label.chars().collect::<Vec<_>>().as_slice() {
        return ui.bound_key(*key).to_string();
    }
    if label == "H/L" {
        return format!("{}/{}", ui.bound_key('H'), ui.bound_key('L'));
    }
    label
        .split(' ')
        .map(|token| {
            if token != "/" && token.len() == 1 {
                token.chars().map(|c| ui.bound_key(c)).collect()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
