use std::path::Path;

use corral_core::Store;

/// A fixture is accepted only from the directory the store's `[agent] assets_dir` names.
pub(super) fn explicit_assets(root: &Path, assets: &Path) -> bool {
    let Ok(store) = Store::open_unobserved(root) else {
        return false;
    };
    let canonical = |p: &Path| p.canonicalize().ok();
    store
        .config
        .agent
        .assets_dir
        .as_deref()
        .and_then(canonical)
        .is_some_and(|dir| Some(dir) == canonical(assets))
}
