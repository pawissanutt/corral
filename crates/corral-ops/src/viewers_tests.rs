use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};

use super::{
    Host, Route, SYSTEM_VIEWER, TERMINAL_BROWSER, file_url, is_wsl, open_with, render_template,
    route, system_opener,
};

const WSL_VERSION: &str =
    "Linux version 6.6.114.1-microsoft-standard-WSL2 (root@x) (gcc 13.2.0) #1 SMP";
const LINUX_VERSION: &str =
    "Linux version 6.8.0-45-generic (buildd@lcy02) (gcc 13.2.0) #45-Ubuntu SMP";

/// Scripted host: fixed os, /proc/version, PATH and env; records every command it is asked to run.
struct FakeHost {
    os: &'static str,
    proc_version: Option<&'static str>,
    path: Vec<&'static str>,
    env: Vec<(&'static str, &'static str)>,
    wslpath: Result<String, String>,
    launch_fails: bool,
    outputs: RefCell<Vec<Vec<String>>>,
    launches: RefCell<Vec<(Vec<String>, bool, bool)>>,
}

impl FakeHost {
    fn linux() -> FakeHost {
        FakeHost {
            os: "linux",
            proc_version: Some(LINUX_VERSION),
            path: Vec::new(),
            env: Vec::new(),
            wslpath: Err("wslpath not expected".into()),
            launch_fails: false,
            outputs: RefCell::new(Vec::new()),
            launches: RefCell::new(Vec::new()),
        }
    }

    fn wsl() -> FakeHost {
        FakeHost {
            proc_version: Some(WSL_VERSION),
            wslpath: Ok(r"\\wsl.localhost\Ubuntu\ws\.corral\artifacts\a.pdf".into()),
            ..FakeHost::linux()
        }
    }

    fn in_herdr_with_browser(mut self) -> FakeHost {
        self.path.push("terminal-browser");
        self.env.push(("HERDR_PANE_ID", "w1:p1"));
        self
    }

    fn launched(&self) -> Vec<(Vec<String>, bool, bool)> {
        self.launches.borrow().clone()
    }
}

impl Host for FakeHost {
    fn os(&self) -> &str {
        self.os
    }

    fn proc_version(&self) -> Option<String> {
        self.proc_version.map(str::to_string)
    }

    fn on_path(&self, program: &str) -> bool {
        self.path.contains(&program)
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    }

    fn output(&self, argv: &[String]) -> Result<String> {
        self.outputs.borrow_mut().push(argv.to_vec());
        match (argv.first().map(String::as_str), &self.wslpath) {
            (Some("wslpath"), Ok(out)) => Ok(out.clone()),
            (Some("wslpath"), Err(e)) => bail!("{} failed: {e}", argv.join(" ")),
            _ => bail!("unexpected command {argv:?}"),
        }
    }

    fn launch(&self, argv: &[String], check_status: bool, wait: bool) -> Result<()> {
        self.launches
            .borrow_mut()
            .push((argv.to_vec(), check_status, wait));
        if self.launch_fails {
            bail!("{} exited with exit status: 1", argv.join(" "));
        }
        Ok(())
    }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn viewers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn default_routes_html_to_terminal_browser_only_inside_herdr() {
    let none = BTreeMap::new();
    let herdr = FakeHost::linux().in_herdr_with_browser();
    for f in ["/a/report.html", "/a/page.HTM", "/a/diagram.svg"] {
        assert_eq!(
            route(&none, Path::new(f), &herdr),
            Route::Template(TERMINAL_BROWSER.into()),
            "{f}"
        );
    }
    for f in ["/a/shot.png", "/a/plan.md", "/a/doc.pdf", "/a/noext"] {
        assert_eq!(route(&none, Path::new(f), &herdr), Route::System, "{f}");
    }

    let no_browser = FakeHost {
        env: vec![("HERDR_PANE_ID", "w1:p1")],
        ..FakeHost::linux()
    };
    assert_eq!(
        route(&none, Path::new("/a/r.html"), &no_browser),
        Route::System
    );
    let outside_herdr = FakeHost {
        path: vec!["terminal-browser"],
        ..FakeHost::linux()
    };
    assert_eq!(
        route(&none, Path::new("/a/r.html"), &outside_herdr),
        Route::System
    );
}

#[test]
fn config_overrides_defaults() {
    let cfg = viewers(&[
        ("png", "feh --scale-down {path}"),
        (".pdf", "zathura {path}"),
        ("html", "system"),
    ]);
    let host = FakeHost::linux().in_herdr_with_browser();
    assert_eq!(
        route(&cfg, Path::new("/a/shot.PNG"), &host),
        Route::Template("feh --scale-down {path}".into())
    );
    assert_eq!(
        route(&cfg, Path::new("/a/doc.pdf"), &host),
        Route::Template("zathura {path}".into()),
        "a leading dot in the key is accepted"
    );
    assert_eq!(
        route(&cfg, Path::new("/a/r.html"), &host),
        Route::System,
        "\"system\" beats the terminal-browser default"
    );
    assert_eq!(
        route(&cfg, Path::new("/a/d.svg"), &host),
        Route::Template(TERMINAL_BROWSER.into()),
        "unlisted extensions keep the defaults"
    );
}

#[test]
fn template_rendering_keeps_paths_whole() {
    let p = Path::new("/ws/my files/report #1.html");
    assert_eq!(
        render_template(TERMINAL_BROWSER, p).unwrap(),
        argv(&[
            "terminal-browser",
            "open",
            "--split",
            "right",
            "/ws/my files/report #1.html"
        ])
    );
    assert_eq!(
        render_template("firefox --new-tab {url}", p).unwrap(),
        argv(&[
            "firefox",
            "--new-tab",
            "file:///ws/my%20files/report%20%231.html"
        ])
    );
    assert_eq!(
        render_template(
            r#"viewer --title 'T 22' "--in=\"{path}\"" x{path}y"#,
            Path::new("/a.png")
        )
        .unwrap(),
        argv(&["viewer", "--title", "T 22", r#"--in="/a.png""#, "x/a.pngy"])
    );
    assert!(render_template("  ", p).is_err());
    let err = render_template("open 'oops {path}", p)
        .unwrap_err()
        .to_string();
    assert!(err.contains("unterminated"), "{err}");
    assert_eq!(file_url(Path::new("/a/b-c_d.~e")), "file:///a/b-c_d.~e");
}

#[test]
fn wsl_detection_reads_proc_version() {
    assert!(is_wsl(WSL_VERSION));
    assert!(is_wsl(
        "Linux version 4.4.0-19041-Microsoft (Microsoft@Microsoft.com)"
    ));
    assert!(!is_wsl(LINUX_VERSION));
}

#[test]
fn wsl_without_wslview_converts_the_path_for_explorer() {
    let host = FakeHost::wsl();
    let p = Path::new("/ws/.corral/artifacts/a.pdf");
    let opened = open_with(&BTreeMap::new(), p, &host, false).unwrap();
    assert_eq!(
        *host.outputs.borrow(),
        vec![argv(&["wslpath", "-w", "/ws/.corral/artifacts/a.pdf"])]
    );
    let expected = argv(&[
        "explorer.exe",
        r"\\wsl.localhost\Ubuntu\ws\.corral\artifacts\a.pdf",
    ]);
    assert_eq!(opened.argv, expected);
    assert_eq!(
        host.launched(),
        vec![(expected, false, false)],
        "explorer.exe's exit status is not checked"
    );
}

#[test]
fn wsl_prefers_wslview() {
    let host = FakeHost {
        path: vec!["wslview"],
        ..FakeHost::wsl()
    };
    let p = Path::new("/ws/a.pdf");
    assert_eq!(
        system_opener(p, &host).unwrap(),
        argv(&["wslview", "/ws/a.pdf"])
    );
    assert!(host.outputs.borrow().is_empty(), "no wslpath call");
}

#[test]
fn system_opener_per_platform() {
    let p = Path::new("/ws/a.pdf");
    assert_eq!(
        system_opener(p, &FakeHost::linux()).unwrap(),
        argv(&["xdg-open", "/ws/a.pdf"])
    );
    let mac = FakeHost {
        os: "macos",
        proc_version: None,
        ..FakeHost::linux()
    };
    assert_eq!(
        system_opener(p, &mac).unwrap(),
        argv(&["open", "/ws/a.pdf"])
    );
    let unreadable = FakeHost {
        proc_version: None,
        ..FakeHost::linux()
    };
    assert_eq!(system_opener(p, &unreadable).unwrap()[0], "xdg-open");
}

#[test]
fn failures_name_the_command() {
    let broken_wslpath = FakeHost {
        wslpath: Err("exit status: 1".into()),
        ..FakeHost::wsl()
    };
    let err = format!(
        "{:#}",
        open_with(
            &BTreeMap::new(),
            Path::new("/ws/a.pdf"),
            &broken_wslpath,
            true
        )
        .unwrap_err()
    );
    assert!(err.contains("wslpath -w /ws/a.pdf"), "{err}");

    let host = FakeHost {
        launch_fails: true,
        ..FakeHost::linux().in_herdr_with_browser()
    };
    let err = format!(
        "{:#}",
        open_with(&BTreeMap::new(), Path::new("/ws/r.html"), &host, true).unwrap_err()
    );
    assert!(
        err.contains("`terminal-browser open --split right /ws/r.html`"),
        "{err}"
    );
    assert!(host.launched()[0].1, "status checked for normal viewers");
}

#[test]
fn corral_opener_replaces_defaults_but_not_viewers_config() {
    let host = FakeHost {
        env: vec![
            ("CORRAL_OPENER", "sh /tmp/stub.sh"),
            ("HERDR_PANE_ID", "w1:p1"),
        ],
        path: vec!["terminal-browser"],
        ..FakeHost::wsl()
    };
    let none = BTreeMap::new();
    assert_eq!(
        route(&none, Path::new("/ws/r.html"), &host),
        Route::Opener("sh /tmp/stub.sh".into()),
        "beats the terminal-browser default"
    );
    let opened = open_with(&none, Path::new("/ws/my report.pdf"), &host, true).unwrap();
    assert_eq!(
        opened.argv,
        argv(&["sh", "/tmp/stub.sh", "/ws/my report.pdf"])
    );
    assert_eq!(host.launched(), vec![(opened.argv.clone(), true, true)]);
    assert!(
        host.outputs.borrow().is_empty(),
        "no wslpath for the opener"
    );

    let cfg = viewers(&[("pdf", "zathura {path}"), ("png", SYSTEM_VIEWER)]);
    assert_eq!(
        route(&cfg, Path::new("/ws/a.pdf"), &host),
        Route::Template("zathura {path}".into())
    );
    assert_eq!(route(&cfg, Path::new("/ws/a.png"), &host), Route::System);
}
