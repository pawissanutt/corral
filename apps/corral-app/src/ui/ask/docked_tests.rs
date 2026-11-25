use super::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

fn desktop(mut config: EngineConfig) -> Headless {
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    Headless::start(config).unwrap()
}

#[component]
fn DockFixture() -> impl IntoElement {
    let selected = signal(0);
    let closed = signal(0);
    let room = signal(210.0);
    view()
        .flex_row()
        .w(px(210.0))
        .children((1..=2).map(|n| {
            tab(
                TabFace {
                    test_id: format!("dock-tab-{n}"),
                    label: format!("Session {n}: {}", "Long project and branch name ".repeat(8)),
                    spoken: format!("Full session {n} name and status"),
                    lead: view().size(px(8.0)).into_element(),
                    dim: false,
                },
                move || selected.get() == n,
                move || selected.set(n),
                Some(Box::new(move || closed.set(n))),
                room,
            )
        }))
        .child(view().absolute().top(px(40.0)).child(text(move || {
            format!("picked {}, closed {}", selected.get(), closed.get())
        })))
}

fn bounds(app: &mut Headless, id: &str) -> Bounds<Points> {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("missing {id}"))
        .bounds
}

#[test]
fn long_titles_stay_inside_their_own_tab_buttons() {
    let mut app = desktop(EngineConfig::new(Root::of::<DockFixture>()));
    app.wait_for("id:dock-tab-2").unwrap();
    for n in 1..=2 {
        let select = bounds(&mut app, &format!("dock-tab-{n}"));
        let text = bounds(&mut app, &format!("dock-tab-{n}-text"));
        let close = bounds(&mut app, &format!("dock-tab-{n}-close"));
        assert!(text.size.width.0 > 0.0 && text.right().0 <= select.right().0);
        assert!(select.right().0 <= close.origin.x.0);
        assert_eq!(close.size.width.0, 20.0);
        if n == 1 {
            assert!(close.right().0 <= bounds(&mut app, "dock-tab-2").origin.x.0);
        }
    }
    app.wait_for("label:Full session 2 name and status")
        .unwrap();
    app.hover("id:dock-tab-2-text").unwrap();
    app.wait_for("text:Full session 2 name and status").unwrap();
    app.press("id:dock-tab-2-text").unwrap();
    app.wait_for("text:picked 2, closed 0").unwrap();
    app.press("id:dock-tab-2-close").unwrap();
    app.wait_for("text:picked 2, closed 2").unwrap();
}

#[component]
fn TightDockFixture() -> impl IntoElement {
    use crate::model::{DataState, Form};
    let app = AppState::install(Form::Desktop);
    app.set_data(DataState::Fixture);
    app.dock_session(&Id::from("B-k3x9"));
    app.dock_session(&Id::from("B-code"));
    app.open_ask();
    view()
        .w(px(160.0))
        .child(strip(app))
        .child(text(move || {
            if app.pane_open() {
                "Pane shown"
            } else {
                "Pane hidden"
            }
        }))
        .child(
            view()
                .test_id("choose-last")
                .on_press(move || app.select_dock(&Id::from("B-code")))
                .child(text("Choose last")),
        )
}

#[test]
fn tight_dock_scrolls_only_its_tabs_to_reveal_selection() {
    let mut app = desktop(EngineConfig::new(Root::of::<TightDockFixture>()));
    app.wait_for("id:dock-tab-B-code").unwrap();
    let viewport = bounds(&mut app, "dock-tab-strip");
    let new = bounds(&mut app, "pane-new");
    let close = bounds(&mut app, "pane-close");
    assert!(viewport.right().0 <= new.origin.x.0 && new.right().0 <= close.origin.x.0);
    assert!(bounds(&mut app, "dock-tab-B-code-close").right().0 > viewport.right().0);
    app.press("id:choose-last").unwrap();
    assert!(bounds(&mut app, "dock-tab-B-code").origin.x.0 >= viewport.origin.x.0);
    assert!(bounds(&mut app, "dock-tab-B-code-close").right().0 <= viewport.right().0);
    assert_eq!(new, bounds(&mut app, "pane-new"));
    assert_eq!(close, bounds(&mut app, "pane-close"));
    app.press("id:pane-new").unwrap();
    app.wait_for("id:pane-new-open").unwrap();
    app.press_key("escape").unwrap();
    app.press("id:pane-close").unwrap();
    app.wait_for("text:Pane hidden").unwrap();
}
