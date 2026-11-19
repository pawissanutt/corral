use nsh_core::element::{Runtime, create_root};
use nsh_core::prelude::*;

use super::held_open;

/// The ⋯ menu's count of open menus was a `stored` of the page's owner, read by the owner's
/// cleanup. NSH frees what an owner made before it runs the owner's cleanups, so leaving a
/// session page whose owner was disposed panicked ("a signal read or written after its owner
/// was disposed") and froze the web app. Going with the page, an open menu must release its count
/// once, and a closed one none.
#[test]
fn an_open_menu_s_count_is_released_once_when_its_page_goes() {
    let runtime = Runtime::default();
    let _entered = runtime.enter();
    let menus = signal(0u32);

    let (page, open) = create_root(|| {
        let open = signal(false);
        held_open(menus, open);
        open
    });
    open.set(true);
    assert_eq!(menus.get_untracked(), 1, "an open menu is counted");
    page.dispose();
    assert_eq!(menus.get_untracked(), 0, "its page going releases it once");

    let (page, open) = create_root(|| {
        let open = signal(false);
        held_open(menus, open);
        open
    });
    open.set(true);
    open.set(false);
    assert_eq!(menus.get_untracked(), 0, "closing it releases it");
    page.dispose();
    assert_eq!(
        menus.get_untracked(),
        0,
        "a closed menu releases nothing more"
    );
}
