# Candidate repository guard

Add `"python3 scripts/check-candidate.py"` to the gate command array. The caller
must supply `CORRAL_INTEGRATION_BASE` or use `python3 scripts/check-candidate.py
--base <commit-or-ref>`; the explicit argument wins. There is no default branch
or implicit merge-base. The comparison is the supplied commit's tree versus
HEAD, including all candidate changes, not just the last commit. Run from inside
the candidate repository. Exit 0 means pass; exit 1 means policy/read failure.
Standard output is one JSON object; CLI usage errors use argparse's exit 2.

The guard does not stage, refresh, commit, checkout, or write files. Every Git
call uses argv, null stdin, disabled optional locks and fsmonitor, and no replace
objects. Content comes from HEAD blobs, not filesystem paths: symlinks cannot
escape the repository. Changed symlinks and submodule entries fail explicitly.
The candidate must have no tracked, staged, untracked (nonignored), or submodule
dirt. Dirty files are reported but not scanned as though they belonged to HEAD.
Ignored build outputs are not candidate content.

## Policy `corral-candidate-v1`

- Detect merge, cherry-pick, revert, rebase/apply, sequencer, bisect, and
  AUTO_MERGE state through Git's worktree-aware `git rev-parse --git-path`.
- Inspect every changed regular blob, including extensionless files and dotfiles.
  Text means no NUL byte; decode UTF-8 with surrogate escapes so invalid UTF-8
  does not hide ASCII markers. NUL-containing Rust fails; other binary blobs
  are counted but not scanned. Deleted files have no current content to scan.
- Detect standalone, column-one Git marker lines: seven or more `<`, `>`, or
  `|`, optionally followed by a space and label; or seven or more `=` alone.
  CRLF is supported. No indentation or surrounding code/quotes is stripped.
  Inline quoted marker examples and indented Markdown examples are legitimate.
  Standalone examples, even in comments, strings, or Markdown fences, fail:
  indent or quote them instead. There is deliberately no allowlist or bypass.
- Rename detection uses `git diff --find-renames`. Both old and new names contribute
  to `policy_paths`; either `.rs` suffix applies Rust counting to the destination
  blob. Renaming `large.rs` to `large.txt` cannot bypass the ceiling. A rename
  below Git's similarity threshold appears as deletion/addition; ordinary
  deletion has no current content and an added non-Rust path has no Rust policy.
- All changed Rust blobs must contain **at most 800 SLOC**; exactly 800 passes.
  Count the entire file including tests, not only added diff lines. A source
  line counts once if it has any nonwhitespace code or string content.
  Blank and comment-only lines do not count. `//` comments end at newline;
  `/* ... */` comments nest. Code after a block comment counts.
  Normal strings carry state across lines and recognize escapes. Raw `r`,
  `br`, and `cr` strings recognize matching quotes/hash delimiters (0–255
  hashes). Nonblank string-content lines count even when they resemble
  comments; whitespace-only string lines do not. Character literals, including
  escaped Unicode/hex characters, are consumed as literals; `'a` lifetimes and
  labels are code, not strings. Byte/C normal strings use the same quote
  state. This is a lexical counter, not a Rust parser or compiler gate.

Changing these semantics requires changing the policy identifier. A future
allowlist would alter admission policy, so it must also change this identifier
and be included in the core flow's policy fingerprint. This independent script
does not implement or modify the core flow's fingerprint mechanism.

## JSON evidence

`schema_version`, `policy`, resolved `base` and `head`, `ok`, `counts`,
`rust_files` and `violations` are always present. Counts distinguish diff entries,
distinct policy paths (both names for renames), deleted entries, scanned text,
binary and Rust files, dirty paths, and violations. Each Rust result includes
path, policy paths, SLOC, and limit. Each violation has a `reason`, nullable
`path`, `line`, and `detail`. Findings aggregate rather than stopping at the
first offender; a Git read failure may prevent completing the remaining scan.

Reason codes: `BASE_REQUIRED`, `BASE_INVALID`, `DIRTY_CANDIDATE`,
`GIT_OPERATION_IN_PROGRESS`, `CONFLICT_MARKER`, `RUST_SLOC_EXCEEDED`,
`RUST_BINARY_CONTENT`, `UNSUPPORTED_FILE_MODE`, `GIT_READ_FAILED`.

Run the isolated, stdlib-only tests with:

```sh
python3 scripts/test_check_candidate.py
```

Tests create temporary repositories only. No live herdr session is used.
