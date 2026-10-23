use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::memo::*;
use crate::rev_sha;
use crate::tests::{init, sh};
use crate::work::invocations;

const CHILD_DIR: &str = "CORRAL_MEMO_TEST_CHILD_DIR";
const CHILD_REPO: &str = "CORRAL_MEMO_TEST_CHILD_REPO";
const CHILD_PAIR: &str = "CORRAL_MEMO_TEST_CHILD_PAIR";

/// A history of additions and a modification, all memoizable; returns every commit.
fn history(repo: &Path) -> Vec<String> {
    let mut commits = vec![rev_sha(repo, "HEAD").unwrap()];
    for i in 0..6 {
        match i {
            2 => fs::write(repo.join("a.txt"), "changed\n").unwrap(),
            _ => fs::write(repo.join(format!("f{i}.txt")), format!("{i}\n")).unwrap(),
        }
        sh(repo, &["add", "-A"]);
        sh(repo, &["commit", "-m", &format!("c{i}")]);
        commits.push(rev_sha(repo, "HEAD").unwrap());
    }
    commits
}

#[derive(Debug, PartialEq, Eq)]
struct Facts4 {
    paths: Vec<String>,
    renames: Vec<String>,
    base: String,
    landed: Vec<Vec<u8>>,
}

fn facts(memo: &Memo, repo: &Path, a: &str, b: &str) -> Facts4 {
    Facts4 {
        paths: memo.changed_paths_between(repo, a, b).unwrap(),
        renames: memo.changed_paths_with_renames(repo, a, b).unwrap(),
        base: memo.merge_base(repo, a, b).unwrap(),
        landed: memo.landed_paths(repo, a, b).unwrap(),
    }
}

fn all_facts(memo: &Memo, repo: &Path, commits: &[String]) -> Vec<Facts4> {
    let mut out = Vec::new();
    for (i, a) in commits.iter().enumerate() {
        for b in &commits[i + 1..] {
            out.push(facts(memo, repo, a, b));
        }
    }
    out
}

fn memo_bytes(dir: &Path) -> Vec<u8> {
    let files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "one memo file per repository: {files:?}");
    fs::read(&files[0]).unwrap()
}

fn copy_memo(from: &Path, to: &Path, bytes: &[u8]) {
    let name = fs::read_dir(from)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    fs::create_dir_all(to).unwrap();
    fs::write(to.join(name), bytes).unwrap();
}

#[test]
fn stored_facts_are_served_by_a_fresh_load_without_git() {
    let (tmp, repo) = init();
    let commits = history(&repo);
    let expected = all_facts(&Memo::disabled(), &repo, &commits);
    let fill = tmp.path().join("fill");
    assert_eq!(all_facts(&Memo::new(&fill), &repo, &commits), expected);

    let copy = tmp.path().join("copy");
    copy_memo(&fill, &copy, &memo_bytes(&fill));
    let before = invocations();
    assert_eq!(all_facts(&Memo::new(&copy), &repo, &commits), expected);
    assert_eq!(invocations(), before, "every fact came from the memo");
    assert_eq!(memo_bytes(&copy), memo_bytes(&fill), "nothing was appended");
    assert!(expected[2].landed.contains(&b"a.txt".to_vec()));
}

#[test]
fn deleted_truncated_and_flipped_memos_recompute_identical_results() {
    let (tmp, repo) = init();
    let commits = history(&repo);
    let expected = all_facts(&Memo::disabled(), &repo, &commits);
    let fill = tmp.path().join("fill");
    all_facts(&Memo::new(&fill), &repo, &commits);
    let good = memo_bytes(&fill);

    let deleted = tmp.path().join("deleted");
    let before = invocations();
    assert_eq!(all_facts(&Memo::new(&deleted), &repo, &commits), expected);
    assert!(invocations() > before);

    let mut truncated = good.clone();
    truncated.truncate(good.len() - 3);
    let mut flipped = good.clone();
    let middle = good.len() / 2;
    flipped[middle] ^= 0x01;
    for (name, bytes) in [("truncated", truncated), ("flipped", flipped)] {
        let dir = tmp.path().join(name);
        copy_memo(&fill, &dir, &bytes);
        let file = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let loaded = load(&file).unwrap();
        assert!(loaded.len() < load(&fill.join(file.file_name().unwrap())).unwrap().len());
        let before = invocations();
        assert_eq!(
            all_facts(&Memo::new(&dir), &repo, &commits),
            expected,
            "{name}"
        );
        assert!(
            invocations() > before,
            "{name}: the damaged record was recomputed"
        );
        assert_eq!(
            load(&file).unwrap(),
            load(&fill.join(file.file_name().unwrap())).unwrap()
        );
    }
}

#[test]
fn revspecs_short_shas_and_other_repositories_are_never_served() {
    let (tmp, repo) = init();
    let commits = history(&repo);
    let (a, b) = (&commits[0], &commits[3]);
    let truth = Memo::disabled().changed_paths_between(&repo, a, b).unwrap();
    let dir = tmp.path().join("memo");
    let memo = Memo::new(&dir);
    let slot = memo.slot(&repo, Kind::Paths, a, b).unwrap();
    let file = memo.file_for(&repo).unwrap();
    fs::create_dir_all(&dir).unwrap();
    fs::write(&file, encode_record(&slot.prefix, b"poison\0")).unwrap();
    assert_eq!(memo.changed_paths_between(&repo, a, b).unwrap(), ["poison"]);

    let clone = tmp.path().join("clone");
    sh(
        tmp.path(),
        &[
            "clone",
            "-q",
            repo.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    assert_ne!(memo.file_for(&clone), memo.file_for(&repo));
    assert_eq!(memo.changed_paths_between(&clone, a, b).unwrap(), truth);
    let foreign_dir = tmp.path().join("foreign");
    let foreign = Memo::new(&foreign_dir);
    let foreign_file = foreign.file_for(&clone).unwrap();
    fs::create_dir_all(&foreign_dir).unwrap();
    fs::copy(&file, &foreign_file).unwrap();
    let before = invocations();
    assert_eq!(foreign.changed_paths_between(&clone, a, b).unwrap(), truth);
    assert!(
        invocations() > before,
        "a record copied from another repository is not served"
    );

    let poisoned = fs::read(&file).unwrap();
    let short = (&a[..12], &b[..12]);
    for (older, newer) in [short, ("HEAD~3", "HEAD"), (a.as_str(), "main")] {
        let before = invocations();
        assert!(memo.slot(&repo, Kind::Paths, older, newer).is_none());
        assert!(memo.changed_paths_between(&repo, older, newer).is_err());
        assert!(
            memo.changed_paths_with_renames(&repo, older, newer)
                .is_err()
        );
        assert!(memo.merge_base(&repo, older, newer).is_err());
        let landed = memo.landed_paths(&repo, older, newer).unwrap();
        assert!(!landed.contains(&b"poison".to_vec()));
        assert!(
            invocations() > before,
            "{older} {newer} was computed by git"
        );
    }
    assert_eq!(fs::read(&file).unwrap(), poisoned, "nothing was memoized");
    assert_eq!(
        memo.landed_paths(&repo, short.0, short.1).unwrap(),
        Memo::disabled().landed_paths(&repo, a, b).unwrap()
    );
}

fn release_after_ready(
    children: &mut [std::process::Child],
) -> Vec<BufReader<std::process::ChildStdout>> {
    let mut readers = Vec::new();
    for child in children.iter_mut() {
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.trim_end().ends_with("MEMO-READY") {
            line.clear();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "child reached the barrier"
            );
        }
        readers.push(reader);
    }
    for child in children.iter_mut() {
        drop(child.stdin.take());
    }
    readers
}

/// Two processes fill one memo concurrently, released together by a stdin barrier; a reader
/// loads the file throughout and must only ever see complete records with reference payloads.
#[test]
fn concurrent_processes_append_only_valid_records() {
    if let (Some(dir), Some(repo)) = (std::env::var_os(CHILD_DIR), std::env::var_os(CHILD_REPO)) {
        let repo = PathBuf::from(repo);
        let memo = Memo::new(PathBuf::from(dir));
        let commits: Vec<_> = (0..=6)
            .rev()
            .map(|k| rev_sha(&repo, &format!("HEAD~{k}")).unwrap())
            .collect();
        assert!(
            memo.slot(&repo, Kind::Paths, &commits[0], &commits[1])
                .unwrap()
                .get()
                .is_none()
        );
        println!("MEMO-READY");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
        all_facts(&memo, &repo, &commits);
        return;
    }
    let (tmp, repo) = init();
    let commits = history(&repo);
    let reference_dir = tmp.path().join("reference");
    all_facts(&Memo::new(&reference_dir), &repo, &commits);
    let name = fs::read_dir(&reference_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    let reference = load(&reference_dir.join(&name)).unwrap();

    let shared = tmp.path().join("shared");
    let mut children: Vec<_> = (0..2)
        .map(|_| {
            Command::new(std::env::current_exe().unwrap())
                .args(["memo_tests::concurrent_processes_append_only_valid_records"])
                .args(["--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD_DIR, &shared)
                .env(CHILD_REPO, &repo)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut readers = release_after_ready(&mut children);
    let file = shared.join(&name);
    let mut running = true;
    while running {
        running = children.iter_mut().any(|c| c.try_wait().unwrap().is_none());
        for (prefix, payload) in load(&file).unwrap() {
            assert_eq!(
                reference.get(&prefix),
                Some(&payload),
                "torn or foreign record served"
            );
        }
    }
    for (mut child, reader) in children.into_iter().zip(&mut readers) {
        reader.read_to_end(&mut Vec::new()).unwrap();
        assert!(child.wait().unwrap().success());
    }
    let bytes = fs::read(&file).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    let records: Vec<_> = bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    assert!(records.len() >= reference.len());
    assert!(
        records.iter().all(|line| decode_record(line).is_some()),
        "every line is valid"
    );
    assert_eq!(load(&file).unwrap(), reference);
}

/// The pre-memo lesson command, run directly; sorted like `landed_paths`.
fn name_only(repo: &Path, a: &str, b: &str) -> Vec<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            a,
            b,
            "--",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = out.stdout.strip_suffix(&[0]).unwrap_or(&out.stdout);
    let mut names: Vec<_> = text
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    names.sort();
    names
}

/// Every pair's memoized facts equal the uncached ones and landed paths equal the original command.
fn assert_current(memo: &Memo, repo: &Path, commits: &[String], step: &str) {
    for (i, a) in commits.iter().enumerate() {
        for b in &commits[i + 1..] {
            let uncached = facts(&Memo::disabled(), repo, a, b);
            assert_eq!(facts(memo, repo, a, b), uncached, "{step}: {a} {b}");
            assert_eq!(uncached.landed, name_only(repo, a, b), "{step}: {a} {b}");
        }
    }
}

fn set(repo: &Path, key: &str, value: Option<&str>) {
    match value {
        Some(value) => sh(repo, &["config", key, value]),
        None => sh(repo, &["config", "--unset", key]),
    }
}

/// Fills the memo, then changes every kind of setting that can alter a diff of the same SHAs,
/// in both directions, and requires memoized facts to equal uncached facts in this long-lived
/// process and in a fresh process after each change.
#[test]
fn settings_changes_never_serve_facts_computed_under_other_settings() {
    if let (Some(dir), Some(repo), Some(commits)) = (
        std::env::var_os(CHILD_DIR),
        std::env::var_os(CHILD_REPO),
        std::env::var(CHILD_PAIR).ok(),
    ) {
        let commits: Vec<_> = commits.split(' ').map(str::to_owned).collect();
        let memo = Memo::new(PathBuf::from(dir));
        assert_current(&memo, Path::new(&repo), &commits, "fresh process");
        return;
    }
    let (tmp, repo) = init();
    let mut commits = vec![rev_sha(&repo, "HEAD").unwrap()];
    let mut commit = |message: &str| {
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", message]);
        commits.push(rev_sha(&repo, "HEAD").unwrap());
    };
    fs::write(repo.join("old.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    commit("add old.rs");
    sh(&repo, &["mv", "old.rs", "new.rs"]);
    commit("rename");
    let gitlink = format!("160000,{},submodule", commits[0]);
    sh(&repo, &["update-index", "--add", "--cacheinfo", &gitlink]);
    sh(&repo, &["commit", "-q", "-m", "gitlink"]);
    commits.push(rev_sha(&repo, "HEAD").unwrap());
    fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    sh(&repo, &["add", "a.txt"]);
    sh(&repo, &["commit", "-q", "-m", "modify"]);
    commits.push(rev_sha(&repo, "HEAD").unwrap());
    let crlf: String = (0..100).map(|i| format!("line {i}\r\n")).collect();
    fs::write(repo.join("old.txt"), &crlf).unwrap();
    sh(&repo, &["add", "old.txt"]);
    sh(&repo, &["commit", "-q", "-m", "crlf text"]);
    commits.push(rev_sha(&repo, "HEAD").unwrap());
    sh(&repo, &["rm", "-q", "old.txt"]);
    fs::write(repo.join("new.txt"), crlf.replace("\r\n", "\n")).unwrap();
    sh(&repo, &["add", "new.txt"]);
    sh(&repo, &["commit", "-q", "-m", "delete crlf, add lf"]);
    commits.push(rev_sha(&repo, "HEAD").unwrap());
    sh(&repo, &["branch", "filtered"]);

    let dir = tmp.path().join("memo");
    let memo = Memo::new(&dir);
    let fresh_process = |checkout: &Path, step: &str| {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["memo_tests::settings_changes_never_serve_facts_computed_under_other_settings"])
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_DIR, &dir)
            .env(CHILD_REPO, checkout)
            .env(CHILD_PAIR, commits.join(" "))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "{step}: a fresh process serves only current facts"
        );
    };
    let check = |checkout: &Path, step: &str| {
        assert_current(&memo, checkout, &commits, step);
        fresh_process(checkout, step);
    };
    check(&repo, "default");
    let rename_pair = facts(&Memo::disabled(), &repo, &commits[1], &commits[2]);
    assert_eq!(rename_pair.landed, [b"new.rs".to_vec()]);

    set(&repo, "diff.renames", Some("false"));
    check(&repo, "diff.renames=false");
    let rename_pair = facts(&Memo::disabled(), &repo, &commits[1], &commits[2]);
    assert_eq!(rename_pair.landed, [b"new.rs".to_vec(), b"old.rs".to_vec()]);
    set(&repo, "diff.ignoreSubmodules", Some("all"));
    check(&repo, "diff.ignoreSubmodules=all");
    set(&repo, "diff.ignoreSubmodules", None);
    set(&repo, "diff.renames", None);
    check(&repo, "local settings removed");

    let gitmodules =
        "[submodule \"submodule\"]\n\tpath = submodule\n\turl = ./other\n\tignore = all\n";
    fs::write(repo.join(".gitmodules"), gitmodules).unwrap();
    let hidden = facts(&Memo::disabled(), &repo, &commits[2], &commits[3]);
    assert!(
        !hidden.paths.contains(&"submodule".to_owned()),
        "the setting applies"
    );
    check(&repo, ".gitmodules ignore=all");
    fs::remove_file(repo.join(".gitmodules")).unwrap();
    check(&repo, ".gitmodules removed");

    let target = tmp.path().join("target.cfg");
    let link = tmp.path().join("link.cfg");
    fs::write(&target, "[diff]\n\tignoreSubmodules = none\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    set(&repo, "include.path", Some(link.to_str().unwrap()));
    check(&repo, "symlinked include, none");
    fs::write(&target, "[diff]\n\tignoreSubmodules = all\n").unwrap();
    check(&repo, "symlinked include target rewritten to all");
    fs::write(&target, "[diff]\n\tignoreSubmodules = none\n").unwrap();
    check(&repo, "symlinked include target rewritten to none");
    set(&repo, "include.path", None);

    let branch_cfg = tmp.path().join("branch.cfg");
    fs::write(&branch_cfg, "[diff]\n\tignoreSubmodules = all\n").unwrap();
    set(
        &repo,
        "includeIf.onbranch:filtered.path",
        branch_cfg.to_str(),
    );
    check(&repo, "onbranch include on main");
    sh(&repo, &["checkout", "-q", "filtered"]);
    check(&repo, "onbranch include on filtered");
    sh(&repo, &["checkout", "-q", "main"]);
    check(&repo, "onbranch include back on main");

    let hostile = [
        ("diff.external", "false"),
        ("diff.boom.textconv", "false"),
        ("diff.renameLimit", "1"),
        ("diff.relative", "true"),
        ("diff.noprefix", "true"),
        ("core.abbrev", "12"),
    ];
    fs::write(repo.join(".gitattributes"), "* diff=boom\n").unwrap();
    for (key, value) in hostile {
        set(&repo, key, Some(value));
    }
    check(&repo, "hostile diff drivers, abbreviation and rename limit");
    for (key, _) in hostile {
        set(&repo, key, None);
    }
    fs::remove_file(repo.join(".gitattributes")).unwrap();
    check(&repo, "hostile settings removed");

    let (crlf, lf) = (&commits[5], &commits[6]);
    let landed = |repo: &Path| Memo::disabled().landed_paths(repo, crlf, lf).unwrap();
    fs::write(repo.join(".gitattributes"), "* -diff\n").unwrap();
    assert_eq!(
        landed(&repo).len(),
        2,
        "binary classification hides the rename"
    );
    check(&repo, ".gitattributes * -diff");
    fs::remove_file(repo.join(".gitattributes")).unwrap();
    assert_eq!(
        landed(&repo),
        [b"new.txt".to_vec()],
        "text classification finds it"
    );
    check(&repo, ".gitattributes removed");
    let info = repo.join(".git/info/attributes");
    fs::create_dir_all(info.parent().unwrap()).unwrap();
    fs::write(&info, "* -diff\n").unwrap();
    check(&repo, "info/attributes * -diff");
    fs::remove_file(&info).unwrap();
    check(&repo, "info/attributes removed");
    fs::write(repo.join(".gitattributes"), "* diff=custom\n").unwrap();
    for binary in ["true", "false", "true"] {
        set(&repo, "diff.custom.binary", Some(binary));
        check(&repo, &format!("diff.custom.binary={binary}"));
    }
    set(&repo, "diff.custom.binary", None);
    fs::remove_file(repo.join(".gitattributes")).unwrap();
    check(&repo, "custom driver removed");

    let worktree = tmp.path().join("linked");
    sh(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().unwrap(),
            "main",
        ],
    );
    fs::write(worktree.join(".gitmodules"), gitmodules).unwrap();
    assert_eq!(
        memo.file_for(&worktree),
        memo.file_for(&repo),
        "one shared memo file"
    );
    check(&worktree, "linked worktree with .gitmodules ignore=all");
    check(&repo, "main worktree without .gitmodules");

    let modify = facts(&Memo::disabled(), &repo, &commits[3], &commits[4]);
    assert_eq!(
        modify.paths,
        ["a.txt"],
        "the gitlink is unchanged after it lands"
    );
    let served = invocations();
    for (a, b) in [(0, 1), (3, 4)] {
        facts(&memo, &repo, &commits[a], &commits[b]);
    }
    assert_eq!(
        invocations(),
        served,
        "settings-free pairs are served from the memo"
    );
    facts(&memo, &repo, &commits[2], &commits[3]);
    assert!(invocations() > served, "a gitlink pair is always computed");
}
