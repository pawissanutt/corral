# Candidate review annotations

Annotations are durable owner-authored review threads attached to **committed candidate code**.
Use [task comments](comments.md) for task-wide discussion. Annotations are review context, not
verification evidence: they do not enter dispatch/reviewer prompts, review packets or digests,
spec revisions, candidate identities, gate receipts, completion audits or acceptance. Open
threads block neither completion nor landing. There is no board UI or agent annotation tool.

## Read the candidate, then open a thread

Run mutations from an owner shell outside run worktrees. First read the candidate identity:

```sh
corral annotation list T-041 --json
```

The v1 view contains `candidate.status`: `available` with `candidate.run`, `base`, `head` and
`tree`, or `unavailable` with a reason (`no_run`, `worktree_missing`, `git_failed`). Copy the
observed run/base/head into the open command; do not infer the run from a matching commit.

```sh
corral annotation open T-041 --run R1 --base <base-sha> --head <head-sha> --path src/lib.rs --lines 10-12 --text 'Check the bound' --json
corral annotation reply T-041 N1 --revision 1 --text-file review-note.txt --json
corral annotation resolve T-041 N1 --revision 2 --text 'Addressed' --json
corral annotation reopen T-041 N1 --revision 3 --json
corral annotation list T-041 --state open --anchor current --limit 16 --json
corral annotation show T-041 N1 --limit 16 --json
```

Open and reply require non-whitespace note text: choose `--text` or a regular UTF-8
`--text-file` (not stdin). Resolve and reopen accept an optional note. `--revision` is the
thread revision you read, not a task revision. Replies to resolved threads leave them resolved;
resolve and reopen are explicit transitions. No edit, delete or relocate operation exists.

Paths must be repository-relative, outside `.git`, and changed between the merge base and
committed head. `--old` selects the base-side blob; by default the new/head side is used.
Omit `--lines` for a file-level anchor, or pass a 1-based inclusive `A-B` range. Worktree
edits and untracked files cannot be annotated. Each immutable anchor records task/run/base/
head/tree, path/side/blob, optional range, excerpt and SHA-256 of the full anchored bytes.

## Retries and concurrent actions

Every mutation accepts `--request-key`. Without it the adapter uses `cli-` plus the first
32 hexadecimal digits of the SHA-256 of the normalized canonical request. An identical rerun
therefore returns the original `{task, thread, entry, revision, state, replayed: true}` result,
without advancing timestamps, revisions or events, even if the candidate later moves or its
worktree disappears. A changed payload with the same key is refused. For an intentional new
action with otherwise identical fields, supply a fresh key. Keys are 1..128 ASCII letters,
digits or `._:-`. They bind action, target, observed candidate/revision and text, not a host
generation. Authorization is checked again before every replay and inside the transaction.

A new open with an outdated run/base/head refuses `stale_candidate`, reporting the current
candidate. Reply/resolve/reopen with an outdated revision refuse `thread_revision_conflict`,
reporting the current revision and state. Read again before deciding what to submit next;
never silently retarget a retry.

## Read views and anchor statuses

List returns thread summaries, not entries. Show returns immutable entry pages, including
owner surface, timestamp, kind, text and request identity. Both use the shared `AnnotationsView`
v1 JSON contract and are read-only. Each compact JSON page is at most 32 KiB (before the CLI's
terminating newline). The default item limit is 16, maximum 64; the byte bound can shorten a
page. Continue with the returned `next_cursor` using `--cursor` and the same query/filter.
Cursors freeze membership and upper revisions so concurrent replies appear on a new traversal,
not midway through an existing one. `paths_truncated` explicitly reports omitted candidate paths.

Each thread reports one derived anchor status, without moving its anchor:

| Status | Meaning |
|---|---|
| `current` | Run, merge base, committed head and tree still match. |
| `stale` | Same run, but base/head/tree changed; structured differences name expected and actual values. Every new commit makes earlier threads stale. |
| `superseded` | A newer run exists, even when its worktree is unavailable. |
| `unavailable` | The current candidate cannot be verified. The stored excerpt remains readable. |

Candidate availability is separate from anchor status. Stale/superseded threads remain listed
unless excluded by `--anchor`; `--state` accepts `open` or `resolved`. To review new code, open
a new thread with a fresh candidate identity and request key.

## Authorization and refusals

Only the owner can mutate via the CLI. `AI_AGENT` or `PI_CODING_AGENT` set (even empty),
`CORRAL_PM=1`, assistant shells and run-worktree cwds refuse `not_owner` without a write.
List/show remain readable from agent/PM/run shells; assistant reads retain their normal session
admission. Owner authors are derived from authenticated entry attribution, never an input flag.
Pi panel authorship is owner via an authenticated host-only surface, not the pane's agent.

Refusals exit nonzero. With `--json`, stderr contains
`{"error":{"code":"thread_revision_conflict","message":"thread revision moved","detail":{"revision":2,"state":"resolved"}}}`;
success JSON goes to stdout. Infrastructure failures use `internal_error`; argument parsing
failures use `bad_request`. Service refusal codes are:

| Codes | Cause |
|---|---|
| `not_owner`, `role_not_permitted`, `out_of_scope` | Caller lacks authority or scope. |
| `stale_candidate`, `candidate_not_current`, `candidate_unavailable` | Candidate changed, is not current, or cannot be verified. |
| `invalid_path`, `path_out_of_scope` | Illegal path or absent/unchanged requested side. |
| `invalid_range`, `binary_range`, `file_too_large` | Invalid lines, binary/non-UTF-8 line anchor, or oversized line-anchor blob. |
| `invalid_text`, `annotation_limit` | Invalid note/key, or a hard capacity bound. |
| `thread_not_found`, `thread_revision_conflict`, `invalid_transition` | Missing thread, concurrent update, or already-resolved/already-open transition. |
| `request_key_conflict` | Retry key binds different payload. |
| `unsupported_version`, `invalid_cursor` | Unknown record version or invalid/mismatched page traversal. |

## Hard bounds and persistence

Bounds are refused, never evicted or silently truncated:

- 4 KiB UTF-8 note text per entry; 64 entries per thread.
- 256 threads and 256 KiB total note text per task.
- 400 lines per inclusive range; 1 KiB per path.
- Stored excerpt: at most 40 lines and 2 KiB, with an explicit truncation flag. The digest
  covers all anchored bytes, not only the excerpt.
- Line anchors require text blobs at most 8 MiB; file-level hashes stream larger blobs.

Canonical thread metadata and append-only entry rows live in `db/corral.db` under `.corral`,
with task/thread/entry and request-key indexes. Bounded head counters avoid loading entries on
hot task reads. There is no annotation sidecar or events-only authority. A successful action
commits its rows, retry identity, head summary and [informational event](events.md) together.
