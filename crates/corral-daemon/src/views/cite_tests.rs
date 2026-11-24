//! A cited artifact read through `DetailRef::Artifact` resolves as the engine resolves it.

use std::fs;

use corral_engine::ArtifactKind;
use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, DetailReply};

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

/// Resolving a citation sends none of its content, and a cited section longer than one page
/// pages through its window of the blob: the pages join to exactly what the engine cuts.
#[test]
fn a_citation_resolves_without_content_and_a_long_section_pages_from_its_window() {
    let fixture = Fixture::new();
    let task = fixture.named_task("long", None);
    let engine = fixture.shared.engine().unwrap();
    let source = format!(
        "# Intro\nhi\n# Long\n{}# After\nz\n",
        "a line of the long section\n".repeat(1500)
    );
    let path = fixture.shared.root().join("long.md");
    fs::write(&path, &source).unwrap();
    let artifact = engine
        .add_artifact(&task, &path, ArtifactKind::Reference, None)
        .unwrap();
    let cited = format!("{}#long", artifact.reference());
    let expected = engine.show_artifact(&cited).unwrap();
    let get = |target, page| {
        detail(
            &fixture.shared,
            DetailGet {
                target,
                page,
                etag: None,
            },
        )
    };
    let reply = get(
        DetailRef::Citation {
            reference: cited.clone(),
        },
        None,
    )
    .unwrap();
    let Some(DetailBody::Citation(found)) = reply.body else {
        panic!("expected a citation");
    };
    assert_eq!(found.artifact.reference(), artifact.reference());
    assert_eq!(found.section_bytes, Some(expected.len() as u64));
    let absent = DetailRef::Citation {
        reference: format!("{}#absent", artifact.reference()),
    };
    assert!(get(absent, None).unwrap_err().is::<NotFound>());

    let target = DetailRef::Artifact { reference: cited };
    let (mut page, mut joined, mut pages) = (None, String::new(), 0);
    loop {
        let reply = get(target.clone(), page).unwrap();
        let Some(DetailBody::Artifact(part)) = reply.body else {
            panic!("expected a page");
        };
        joined.push_str(&part.content);
        pages += 1;
        match reply.next {
            Some(next) => page = Some(next),
            None => break,
        }
    }
    assert!(pages > 1, "the section spans pages");
    assert_eq!(joined.as_bytes(), expected.as_slice());
}

/// A store holding one cited artifact: the same bytes in every store, attached under
/// `filename`, whose extension makes it Markdown or not in that store's catalog.
fn store_with(filename: &str, source: &str) -> (Fixture, String) {
    let fixture = Fixture::new();
    let task = fixture.named_task("cited", None);
    let engine = fixture.shared.engine().unwrap();
    let path = fixture.shared.root().join(filename);
    fs::write(&path, source).unwrap();
    let artifact = engine
        .add_artifact(&task, &path, ArtifactKind::Reference, None)
        .unwrap();
    (fixture, format!("{}#goal", artifact.reference()))
}

fn ask(fixture: &Fixture, target: DetailRef, page: Option<String>) -> anyhow::Result<DetailReply> {
    detail(
        &fixture.shared,
        DetailGet {
            target,
            page,
            etag: None,
        },
    )
}

/// One daemon process can serve several stores. The same reference, bytes and slug resolve by
/// each store's own catalog (Markdown in one, TOML in the other) whichever store asks first,
/// and within one store paging a section asks the engine to find it once.
#[test]
fn a_section_decision_stays_in_its_store_and_is_made_once_there() {
    let source = format!("# Goal\n{}", "a line of the goal\n".repeat(1000));
    for markdown_first in [true, false] {
        let (markdown, cited) = store_with("design.md", &source);
        let (toml, same) = store_with("settings.toml", &source);
        assert_eq!(cited, same, "both stores cite the same reference");
        let citation = || DetailRef::Citation {
            reference: cited.clone(),
        };
        let order = if markdown_first {
            [&markdown, &toml]
        } else {
            [&toml, &markdown]
        };
        for fixture in order {
            let answer = ask(fixture, citation(), None);
            if std::ptr::eq(fixture, &markdown) {
                assert!(answer.is_ok(), "Markdown store: {answer:?}");
            } else {
                assert!(answer.unwrap_err().is::<NotFound>(), "TOML store");
            }
        }
        let before = super::citation::PARSED.with(std::cell::Cell::get);
        let target = DetailRef::Artifact {
            reference: cited.clone(),
        };
        let (mut page, mut pages) = (None, 0);
        loop {
            let reply = ask(&markdown, target.clone(), page).unwrap();
            pages += 1;
            match reply.next {
                Some(next) => page = Some(next),
                None => break,
            }
        }
        assert!(pages > 1, "the section spans pages");
        assert_eq!(
            super::citation::PARSED.with(std::cell::Cell::get),
            before,
            "paging reuses the store's decision"
        );
    }
}
