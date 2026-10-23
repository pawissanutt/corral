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

/// A history with additions, a rename and modifications; returns every commit in order.
fn history(repo: &Path) -> Vec<String> {
    let mut commits = vec![rev_sha(repo, "HEAD").unwrap()];
    for i in 0..6 {
        match i {
            2 => sh(repo, &["mv", "a.txt", "moved.txt"]),
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
    let renamed = &expected[2 * commits.len() - 2];
    assert!(renamed.renames.contains(&"a.txt".to_owned()));
    assert!(renamed.landed.contains(&b"moved.txt".to_vec()));
    assert!(!renamed.landed.contains(&b"a.txt".to_vec()));
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

/// The pre-memo lesson command, run directly.
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
    text.split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn assert_current(memo: &Memo, repo: &Path, a: &str, b: &str) -> Facts4 {
    let uncached = facts(&Memo::disabled(), repo, a, b);
    assert_eq!(facts(memo, repo, a, b), uncached);
    assert_eq!(uncached.landed, name_only(repo, a, b));
    uncached
}

#[test]
fn config_changes_never_serve_facts_computed_under_other_settings() {
    if let (Some(dir), Some(repo), Some(pair)) = (
        std::env::var_os(CHILD_DIR),
        std::env::var_os(CHILD_REPO),
        std::env::var(CHILD_PAIR).ok(),
    ) {
        let (a, b) = pair.split_once(' ').unwrap();
        assert_current(&Memo::new(PathBuf::from(dir)), Path::new(&repo), a, b);
        return;
    }
    let (tmp, repo) = init();
    fs::write(repo.join("old.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-m", "old"]);
    let a = rev_sha(&repo, "HEAD").unwrap();
    sh(&repo, &["mv", "old.rs", "new.rs"]);
    let gitlink = format!("160000,{a},submodule");
    sh(&repo, &["update-index", "--add", "--cacheinfo", &gitlink]);
    sh(&repo, &["commit", "-m", "rename and gitlink"]);
    let b = rev_sha(&repo, "HEAD").unwrap();
    let dir = tmp.path().join("memo");
    let memo = Memo::new(&dir);
    let fresh_process = || {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["memo_tests::config_changes_never_serve_facts_computed_under_other_settings"])
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_DIR, &dir)
            .env(CHILD_REPO, &repo)
            .env(CHILD_PAIR, format!("{a} {b}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "a fresh process serves only current facts"
        );
    };
    let default = assert_current(&memo, &repo, &a, &b);
    assert_eq!(default.landed, [b"new.rs".to_vec(), b"submodule".to_vec()]);
    fresh_process();

    sh(&repo, &["config", "diff.renames", "false"]);
    let no_renames = assert_current(&memo, &repo, &a, &b);
    assert_eq!(
        no_renames.landed,
        [
            b"new.rs".to_vec(),
            b"old.rs".to_vec(),
            b"submodule".to_vec()
        ]
    );
    fresh_process();

    sh(&repo, &["config", "diff.ignoreSubmodules", "all"]);
    let ignored = assert_current(&memo, &repo, &a, &b);
    assert!(!ignored.paths.contains(&"submodule".to_owned()));
    assert!(!ignored.renames.contains(&"submodule".to_owned()));
    fresh_process();

    sh(&repo, &["config", "--unset", "diff.ignoreSubmodules"]);
    sh(&repo, &["config", "--unset", "diff.renames"]);
    let before = invocations();
    assert_eq!(assert_current(&memo, &repo, &a, &b), default);
    fresh_process();
    assert!(invocations() > before);
}
