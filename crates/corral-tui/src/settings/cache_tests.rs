use std::cell::Cell;

use corral_core::{ConfigLayers, Store};

use super::SettingsView;
use super::tests::{c, fixture, render_at, type_value};
use crate::{Action, Tone, View};

thread_local! {
    static LOADS: Cell<usize> = const { Cell::new(0) };
}

fn counted_loader(store: &Store) -> anyhow::Result<ConfigLayers> {
    LOADS.set(LOADS.get() + 1);
    store.config_layers()
}

#[test]
fn draws_and_unchanged_refreshes_do_not_load_or_validate_config() {
    // Given a counted real layer loader and an open Settings screen.
    let fx = fixture();
    LOADS.set(0);
    let mut view = SettingsView::with_layer_loader(&fx.ctx, counted_loader);
    assert_eq!(LOADS.get(), 1);
    let validations = view.validations;
    // When every section redraws at narrow and wide sizes between unchanged syncs.
    for width in [80, 160] {
        for section in '1'..='8' {
            view.on_key(c(section), &fx.ctx);
            render_at(&mut view, &fx.ctx, width, 45);
            view.refresh(&fx.ctx);
        }
    }
    // Then neither layer loading nor validation runs again.
    assert_eq!(LOADS.get(), 1);
    assert_eq!(view.validations, validations);
}

#[test]
fn changed_config_reloads_layers_and_validates_only_once() {
    // Given an external semantic edit and an explicit store reload.
    let mut fx = fixture();
    LOADS.set(0);
    let mut view = SettingsView::with_layer_loader(&fx.ctx, counted_loader);
    let validations = view.validations;
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("limits.running", "17"))
        .unwrap();
    fx.ctx.reload_config().unwrap();
    // When Settings receives the changed snapshot followed by an unchanged one.
    view.refresh(&fx.ctx);
    view.refresh(&fx.ctx);
    // Then the new effective config is installed and validated exactly once.
    assert_eq!(view.draft.limits.running, 17);
    assert_eq!(view.base, fx.ctx.store.config);
    assert_eq!(LOADS.get(), 2);
    assert_eq!(view.validations, validations + 1);
}

#[test]
fn saving_reloads_cached_layers_before_the_store_snapshot_catches_up() {
    // Given a staged edit in Settings.
    let fx = fixture();
    LOADS.set(0);
    let mut view = SettingsView::with_layer_loader(&fx.ctx, counted_loader);
    type_value(&mut view, &fx.ctx, "19");
    // When it is saved without replacing the caller's store snapshot.
    let action = view.on_key(c('s'), &fx.ctx);
    // Then the view reloads provenance immediately and keeps its saved base.
    assert!(matches!(action, Action::Toast(t) if t.tone == Tone::Good));
    assert_eq!(LOADS.get(), 2);
    assert_eq!(view.base.limits.running, 19);
    assert!(!view.dirty());
    render_at(&mut view, &fx.ctx, 160, 45);
    assert_eq!(LOADS.get(), 2);
}

#[test]
fn layer_load_errors_are_cached_across_draws() {
    // Given an unavailable layer loader.
    let fx = fixture();
    LOADS.set(0);
    let mut view = SettingsView::with_layer_loader(&fx.ctx, |_| {
        LOADS.set(LOADS.get() + 1);
        anyhow::bail!("counted unavailable layers")
    });
    // When the view redraws and receives an unchanged refresh.
    let screen = render_at(&mut view, &fx.ctx, 160, 45);
    view.refresh(&fx.ctx);
    render_at(&mut view, &fx.ctx, 80, 45);
    // Then the unavailable state is shown without retrying in draw.
    assert!(screen.contains("source: unavailable"), "{screen}");
    assert!(
        screen.contains("unavailable: counted unavailable layers"),
        "{screen}"
    );
    assert_eq!(LOADS.get(), 1);
}

#[test]
fn rejected_reload_refreshes_sources_without_revalidating_the_retained_config() {
    // Given a valid screen and a syntactically invalid external revision.
    let mut fx = fixture();
    let mut view = SettingsView::new(&fx.ctx);
    let validations = view.validations;
    let base = view.base.clone();
    std::fs::write(fx.ctx.store.config_path(), "[ui\n").unwrap();
    assert!(!fx.ctx.reload_config().unwrap());
    // When the rejected revision reaches Settings via refresh.
    view.refresh(&fx.ctx);
    // Then provenance is unavailable but the valid effective config is retained.
    let screen = render_at(&mut view, &fx.ctx, 160, 45);
    assert!(screen.contains("source: unavailable"), "{screen}");
    assert_eq!(view.base, base);
    assert_eq!(view.draft, base);
    assert_eq!(view.validations, validations);
}
