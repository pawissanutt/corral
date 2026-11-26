//! Headless tests of Activity › Artifacts, over the explicit sample preview.

use std::time::Duration;

use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};

/// The finder on a desktop over the sample, filtered to `words`.
fn finder(words: &str) -> Headless {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.viewport = Size::new(Points(1440.0), Points(900.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config.storage = Some(MemoryStorage::new([(
        "corral.destination".to_owned(),
        "artifacts".to_owned(),
    )]));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:palette-open").unwrap();
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:field-artifacts").unwrap();
    app.type_text("id:field-artifacts", words).unwrap();
    app
}

/// A report's Mermaid fence must reach the page as a diagram, not as its source, while the
/// ordinary code fence beside it stays code.
#[test]
fn a_reports_mermaid_fence_draws_as_a_diagram_beside_its_code() {
    let mut app = finder("usage-coverage-report");
    app.wait_for("id:artifact-preview >> id:mermaid-diagram >> id:lens-node-0")
        .unwrap();
    app.wait_for("id:artifact-preview >> text:just check")
        .unwrap();
    assert_eq!(app.screen().count("id:mermaid-source").unwrap(), 0);
}

/// An agent's diagram titled in words (no `.md`, no media type) is Markdown by its own
/// structure: its sequence and ER fences draw on the one canvas and its Rust fence stays code.
#[test]
fn a_descriptively_titled_diagram_draws_its_sequence_and_er_fences() {
    let mut app = finder("How usage reaches");
    app.wait_for("id:artifact-preview >> id:mermaid-diagram >> id:lens-node-0")
        .unwrap();
    app.wait_for(
        "id:artifact-preview >> text:let total: u64 = buckets.iter().map(|b| b.tokens).sum();",
    )
    .unwrap();
    assert_eq!(app.screen().count("id:mermaid-diagram").unwrap(), 2);
    assert_eq!(app.screen().count("id:mermaid-source").unwrap(), 0);
}

/// A standalone HTML report titled in words (no suffix, served as plain text) is a page by its
/// own doctype: it offers Open in browser beside View source and Download.
#[test]
fn a_descriptively_titled_html_report_offers_open_in_browser_and_its_source() {
    let mut app = finder("standalone narrow sidebar");
    app.wait_for("id:btn-open-html-artifact").unwrap();
    app.wait_for("id:btn-download-artifact").unwrap();
    app.wait_for("id:btn-open-full >> text:View source")
        .unwrap();
}
