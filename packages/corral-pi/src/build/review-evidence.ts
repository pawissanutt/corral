import { createHash } from "node:crypto";
import type { Run } from "./corral-cli.ts";
import type { LandingTarget } from "./prepare.ts";
import { readWorkspaceToml } from "./profiles.ts";
import type { RecordReview } from "./record-types.ts";
import type { BuildJournal } from "./record.ts";
import type { BuildContext, PreparedLanding } from "./types.ts";

const sha = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;

export function reviewCoversCandidate(review: RecordReview, candidate: PreparedLanding, journal: Pick<BuildJournal, "reviewContext">): boolean {
  const { evidence } = review;
  if (evidence && evidence.scope !== candidate.reviewScope) return false;
  // Legacy verdicts keep exact-pair coverage, but cannot authorize a replay.
  return (review.head === candidate.head && review.base === candidate.base)
    || (review.verdict === "approve" && evidence?.replay?.head === candidate.head && evidence.replay.base === candidate.base
      && evidence.context === journal.reviewContext().fingerprint);
}

export function reviewPolicy({ session, settings }: BuildContext, target: LandingTarget, branch: string) {
  const { workspaceRoot, project, repoRoot, worktree } = session;
  if (session.bare || !workspaceRoot || !project) return;
  const { build } = readWorkspaceToml(workspaceRoot, "pi.toml") as { build?: { review_neutral_docs?: unknown } };
  const configured = build?.review_neutral_docs;
  const key = `.corral-next/pi.toml: build.review_neutral_docs.${project}`;
  if (configured !== undefined && (!configured || typeof configured !== "object" || Array.isArray(configured))) {
    throw new Error(".corral-next/pi.toml: build.review_neutral_docs must be a table keyed by project.");
  }
  const entry = configured && Object.hasOwn(configured, project) ? (configured as Record<string, unknown>)[project] : [];
  const prose = /^docs\/(?:[a-z0-9][a-z0-9_-]*\/)*[a-z0-9][a-z0-9_-]*\.md$/;
  const control = /(?:^|\/)(?:agents|claude|codex|copilot-instructions|gemini|instructions?|skill)\.md$/;
  if (!Array.isArray(entry) || entry.some(path => typeof path !== "string" || !prose.test(path) || control.test(path))) {
    throw new Error(`${key} must list exact lowercase Markdown paths under docs/, excluding agent and instruction files.`);
  }
  const neutralDocs = new Set<string>([...entry].sort());
  const scope = createHash("sha256").update(JSON.stringify(["corral-build-review-v1", repoRoot, worktree, project, branch,
    target.ref, target.configuredName, target.gateInputFingerprint, settings.exitPolicy, [...neutralDocs]])).digest("hex");
  return { scope, neutralDocs };
}

export async function proveReviewReplay(git: Run, cwd: string, review: RecordReview, source: string, candidate: PreparedLanding,
  neutralDocs: ReadonlySet<string>, context: string): Promise<boolean> {
  const { base, head, evidence } = review;
  if (review.verdict !== "approve" || !base || !sha.test(base) || !sha.test(head) || !evidence
    || evidence.scope !== candidate.reviewScope || evidence.context !== context) return false;
  const anchor = evidence.replay ?? { base, head };
  if (anchor.head !== source || anchor.base === candidate.base || !sha.test(anchor.base)) return false;
  const opts = { cwd, timeoutMs: 5000, env: { GIT_NO_LAZY_FETCH: "1", GIT_TERMINAL_PROMPT: "0" } };
  const forward = await git(["--no-replace-objects", "merge-base", "--is-ancestor", anchor.base, candidate.base], opts);
  if (forward.code !== 0 || forward.stderr) return false;

  // Raw, NUL-delimited changes include complete object IDs and modes. Comparing
  // them has no whitespace, context, rename, textconv, or binary-diff ambiguity.
  const raw = (from: string, to: string) => ["--no-replace-objects", "diff-tree", "-r", "--no-commit-id", "--raw",
    "--no-abbrev", "--no-renames", "--ignore-submodules=none", "-z", from, to, "--"];
  const inputs = await git(raw(base, candidate.base), opts);
  if (inputs.code !== 0 || inputs.stderr) return false;
  if (inputs.stdout) {
    const fields = inputs.stdout.split("\0");
    if (fields.pop() !== "" || fields.length % 2 !== 0) return false;
    for (let i = 0; i < fields.length; i += 2) {
      if (!/^:100644 100644 [0-9a-f]+ [0-9a-f]+ M$/.test(fields[i]!) || !neutralDocs.has(fields[i + 1]!)) return false;
    }
  }
  const [before, after] = await Promise.all([git(raw(base, head), { ...opts, stdoutDigest: "sha256" }),
    git(raw(candidate.base, candidate.head), { ...opts, stdoutDigest: "sha256" })]);
  // A base edit to any file touched by the build changes that file's before-blob ID.
  return before.code === 0 && after.code === 0 && !before.stderr && !after.stderr
    && /^[0-9a-f]{64}$/.test(before.stdout) && before.stdout === after.stdout;
}
