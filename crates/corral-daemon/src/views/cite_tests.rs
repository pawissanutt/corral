//! A cited artifact read through `DetailRef::Artifact` resolves as the engine resolves it.

use std::fs;

use corral_engine::ArtifactKind;
use corral_protocol::detail::{DetailBody, DetailGet, DetailRef};

use super::tests::Fixture;
use super::{NotFound, detail};

/// The app trusts this reply to say whether a citation resolves: a section follows the
/// attachment's source format (not its title), the ordinal is a value (`:03` is `:3`), and
/// every miss is `not_found` rather than a refusal or a read failure.
#[test]
fn a_cited_section_is_served_by_the_engines_rules_and_a_miss_is_not_found() {
    let fixture = Fixture::new();
    let task = fixture.named_task("cited", None);
    let engine = fixture.shared.engine().unwrap();
    let attach = |filename: &str, title: &str| {
        let path = fixture.shared.root().join(filename);
        fs::write(
            &path,
            "# Configuration\nsetting = true\n## Deeper\nx\n# Next\ny\n",
        )
        .unwrap();
        engine
            .add_artifact(&task, &path, ArtifactKind::Reference, Some(title))
            .unwrap()
    };
    let design = attach("design.md", "Design.rev");
    let settings = attach("settings.toml", "design.md");
    let get = |reference: String| {
        detail(
            &fixture.shared,
            DetailGet {
                target: DetailRef::Artifact { reference },
                page: None,
                etag: None,
            },
        )
    };
    let padded = format!("{task}:{:02}#configuration", design.id);
    let Some(DetailBody::Artifact(page)) = get(padded).unwrap().body else {
        panic!("expected the section");
    };
    assert_eq!(
        page.content,
        "# Configuration\nsetting = true\n## Deeper\nx\n"
    );
    // A section far past the first served page is still found: corrald reads the whole source.
    let late = fixture.shared.root().join("late.md");
    fs::write(
        &late,
        format!("{}\n# Late\nfound\n", "filler\n".repeat(2000)),
    )
    .unwrap();
    let late = engine
        .add_artifact(&task, &late, ArtifactKind::Reference, None)
        .unwrap();
    let Some(DetailBody::Artifact(page)) = get(format!("{}#late", late.reference())).unwrap().body
    else {
        panic!("expected the late section");
    };
    assert_eq!(page.content, "# Late\nfound\n");
    for missing in [
        format!("{}#configuration", settings.reference()),
        format!("{}#absent", design.reference()),
        format!("{task}:99"),
    ] {
        let error = get(missing.clone()).unwrap_err();
        assert!(error.is::<NotFound>(), "{missing}: {error:#}");
    }
    let error = detail(
        &fixture.shared,
        DetailGet {
            target: DetailRef::Task { id: "T-999".into() },
            page: None,
            etag: None,
        },
    )
    .unwrap_err();
    assert!(error.is::<NotFound>(), "{error:#}");
}
