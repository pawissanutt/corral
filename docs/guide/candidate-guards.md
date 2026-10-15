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

## Policy `corral-candidate-v3`

The extension map comes from `CORRAL_SIZE_LIMITS` (JSON), exported by corral from the effective
project review policy. `python3 scripts/check-candidate.py --size-limits '{"rs":1000,"ts":400}'`
overrides it for a standalone run.
Without either, the default remains `{"rs":800}`. Limits must be positive integers keyed by
file extensions without dots; configure them under `[projects.<p>.review.size_limits]`.

- Detect merge, cherry-pick, revert, rebase/apply, sequencer and bisect state
  through Git's worktree-aware `git rev-parse --git-path`. A retained AUTO_MERGE
  tree alone is not an active operation; Git can leave it after a clean rebase.
  Unmerged index entries still fail the clean-candidate check.
- Inspect every changed regular blob, including extensionless files and dotfiles.
  Text means no NUL byte; decode UTF-8 with surrogate escapes so invalid UTF-8
  does not hide ASCII markers. NUL-containing configured source files fail; other binary blobs
  are counted but not scanned. Deleted files have no current content to scan.
- Detect standalone, column-one Git marker lines: seven or more `<`, `>`, or
  `|`, optionally followed by a space and label; or seven or more `=` alone.
  CRLF is supported. No indentation or surrounding code/quotes is stripped.
  Inline quoted marker examples and indented Markdown examples are legitimate.
  Standalone examples, even in comments, strings, or Markdown fences, fail:
  indent or quote them instead. There is deliberately no allowlist or bypass.
- Rename detection uses `git diff --find-renames`. Both old and new names contribute
  to `policy_paths`; either name's configured extension applies its ceiling to the destination
  blob. Renaming `large.rs` to `large.txt` cannot bypass a configured Rust ceiling. A rename
  below Git's similarity threshold appears as deletion/addition; ordinary
  deletion has no current content and an added non-Rust path has no Rust policy.
- Each configured source blob must contain **at most its extension's ceiling**; exactly the ceiling passes.
  Rust defaults to 800 SLOC. Other extensions count nonblank lines excluding whole-line `//` comments.
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
`rust_files`, `source_files` and `violations` are always present. Counts distinguish diff entries,
distinct policy paths (both names for renames), deleted entries, scanned text,
binary and Rust files, dirty paths, and violations. Each Rust result includes
path, policy paths, SLOC, and limit. Each violation has a `reason`, nullable
`path`, `line`, and `detail`. Findings aggregate rather than stopping at the
first offender; a Git read failure may prevent completing the remaining scan.

Reason codes: `BASE_REQUIRED`, `BASE_INVALID`, `DIRTY_CANDIDATE`,
`GIT_OPERATION_IN_PROGRESS`, `CONFLICT_MARKER`, `RUST_SLOC_EXCEEDED`,
`RUST_BINARY_CONTENT`, `SOURCE_SLOC_EXCEEDED`, `SOURCE_BINARY_CONTENT`,
`UNSUPPORTED_FILE_MODE`, `GIT_READ_FAILED`.

Run the isolated, stdlib-only tests with:

```sh
python3 scripts/test_check_candidate.py
```

Tests create temporary repositories only. No live herdr session is used.

## Working-tree SLOC check

`python3 scripts/check-candidate.py --worktree-sloc` reuses the same lexical
counter across all tracked and nonignored untracked configured source files on disk, not
only changed HEAD blobs. Deleted tracked files are skipped; nonregular Rust
paths fail. It needs no integration base or clean candidate, leaves gate exports
unchanged, and prints a file count, maximum SLOC and every offender rather than
the candidate JSON report. This mode is only the size check used by
`scripts/gate.sh`; it does not replace candidate admission checks above.
