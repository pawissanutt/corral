use crate::output::{OutputCfg, OutputKind, require_default_cargo_output};

#[test]
fn declarations_are_bounded_private_and_never_cache_or_source_roots() {
    let cargo = OutputCfg {
        kind: OutputKind::CargoTarget,
        path: "target".into(),
        rebuildable: true,
        exclusive: true,
        grace_hours: 24,
        extensions: vec![],
    };
    assert!(cargo.validate().is_ok());
    let web = OutputCfg {
        kind: OutputKind::Generated,
        path: "app/dist".into(),
        extensions: vec!["js".into(), "css".into(), "map".into()],
        ..cargo.clone()
    };
    assert!(web.validate().is_ok());
    for path in [
        ".",
        "../target",
        "/tmp/build",
        "app/../dist",
        "node_modules",
        ".cargo/target",
        "src/output",
        "docs/build",
        ".evidence/dist",
    ] {
        assert!(
            OutputCfg {
                path: path.into(),
                ..web.clone()
            }
            .validate()
            .is_err(),
            "{path}"
        );
    }
    assert!(
        OutputCfg {
            exclusive: false,
            ..web.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        OutputCfg {
            rebuildable: false,
            ..web.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        OutputCfg {
            grace_hours: 0,
            ..web.clone()
        }
        .validate()
        .is_err()
    );
    assert!(
        OutputCfg {
            extensions: vec!["ts".into()],
            ..web
        }
        .validate()
        .is_err()
    );
}

#[test]
fn cargo_configuration_overrides_cannot_hide_behind_toml_spelling() {
    assert!(require_default_cargo_output("[build]\nrustc-wrapper = ''\n").is_ok());
    for text in [
        "[build]\ntarget-dir = '/shared'",
        "[build]\n\"target\\u002ddir\" = '/shared'",
        "[build]\nbuild-dir = '/shared'",
        "[env]\nCARGO_TARGET_DIR = '/shared'",
        "[env]\nCARGO_HOME = '/other/config'",
        "include = ['elsewhere.toml']",
        "not valid TOML!",
    ] {
        assert!(require_default_cargo_output(text).is_err(), "{text}");
    }
}
