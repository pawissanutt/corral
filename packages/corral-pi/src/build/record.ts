import { randomBytes } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checked, type Run } from "./corral-cli.ts";
import { summarize } from "./git.ts";
import type { BuildContext, BuildSessionRecord, ChildSnapshot, FleetSnapshot } from "./types.ts";
import type { BuildGateResult, BuildRecord, RecordDecision, RecordReview, RecordUsage } from "./record-types.ts";

interface JournalData {
  version: 1;
  title?: string;
  summary?: string;
  intent: { at: string; text: string }[];
  ownerSteers: Record<string, number>;
  verification: BuildGateResult[];
  reviews: RecordReview[];
  decisions: RecordDecision[];
  timeline: BuildRecord["timeline"][number][];
  outcome: BuildRecord["outcome"];
  children: ChildSnapshot[];
  mail: FleetSnapshot["mail"];
  commits: BuildRecord["commits"];
  diff?: BuildRecord["diff"];
  branch?: string | null;
  updated: string;
}
interface TranscriptUsage { readonly input?: number; readonly output?: number; readonly cost?: { readonly total?: number } }
export interface BuildJournal {
  record(): BuildRecord;
  liveRecord(): Promise<BuildRecord>;
  refresh(): Promise<void>;
  capture(): void;
  noteOwnerInput(text: string): void;
  prompts(): { at: string; text: string }[];
  finish(title: string, summary?: string): void;
  gate(result: BuildGateResult): void;
  latestGate(): BuildGateResult | undefined;
  /** The latest gate that passed on exactly `head` with exactly `commands`. */
  passedGate(head: string, commands: readonly string[]): BuildGateResult | undefined;
  latestReview(): RecordReview | undefined;
  review(child: ChildSnapshot, head: string): RecordReview;
  resolve(round: number, finding: string, resolution: { state: "fixed" | "declined"; commit?: string; reason?: string }): void;
  decision(kind: RecordDecision["kind"], text: string, answer?: string): void;
  outcome(value: BuildRecord["outcome"]): void;
  timeline(phase: BuildRecord["timeline"][number]["phase"], detail: string): void;
}
const iso = (at: number) => new Date(at).toISOString();
/** Keep the first nonempty request line readable in session lists. */
function requestTitle(text: string): string | undefined {
  const line = text.split(/\r?\n/).map(value => value.trim()).find(Boolean)?.replace(/\s+/g, " ");
  if (!line) return undefined;
  const characters = Array.from(line);
  if (characters.length <= 120) return line;
  const prefix = characters.slice(0, 119).join("");
  const boundary = prefix.lastIndexOf(" ");
  return `${boundary >= 60 ? prefix.slice(0, boundary) : prefix}…`;
}
function sum(values: readonly RecordUsage[]): RecordUsage {
  return { input: values.reduce((n, v) => n + v.input, 0), output: values.reduce((n, v) => n + v.output, 0),
    ...(values.every(v => v.cost !== undefined) ? { cost: values.reduce((n, v) => n + v.cost!, 0) } : {}) };
}

export function createJournal(options: {
  context: BuildContext; git: Run; fleet: () => FleetSnapshot;
  session?: () => BuildSessionRecord; changed?: () => void; now?: () => number;
}): BuildJournal {
  const { context } = options;
  const now = options.now ?? Date.now;
  const session = () => options.session?.() ?? context.session;
  const path = join(context.dir, "journal.json");
  let data: JournalData;
  try {
    data = JSON.parse(readFileSync(path, "utf8")) as JournalData;
    if (data.version !== 1) throw new Error("Unsupported build journal version");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    data = { version: 1, intent: [], ownerSteers: {}, verification: [], reviews: [], decisions: [],
      timeline: [{ at: context.session.created, phase: "started" }], outcome: { state: "open" }, children: [], mail: [], commits: [], updated: context.session.updated };
  }
  function persist() {
    data.updated = iso(now());
    mkdirSync(context.dir, { recursive: true, mode: 0o700 });
    const temporary = join(context.dir, `.journal-${randomBytes(12).toString("hex")}.tmp`);
    try {
      writeFileSync(temporary, `${JSON.stringify(data)}\n`, { flag: "wx", mode: 0o600 });
      renameSync(temporary, path);
      chmodSync(path, 0o600);
    } finally { rmSync(temporary, { force: true }); }
    try { options.changed?.(); } catch { /* Observers do not own journal persistence. */ }
  }
  let cachedParent: { key: string; usage: RecordUsage } | undefined;
  function captureFleet() {
    const fleet = options.fleet();
    const children = new Map(data.children.map(child => [child.id, child]));
    for (const child of fleet.children) {
      if (!children.has(child.id)) data.timeline.push({ at: iso(child.createdAt), phase: "children", detail: `${child.id}: ${child.title}` });
      children.set(child.id, child);
      for (let index = data.ownerSteers[child.id] ?? 0; index < child.steers.length; index++) {
        data.intent.push({ at: iso(child.steeredAt?.[index] ?? now()), text: child.steers[index]! });
      }
      data.ownerSteers[child.id] = child.steers.length;
    }
    data.children = [...children.values()];
    data.intent.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
    // A resumed fleet restarts its sequence, so retain previous messages independently.
    const mail = new Map(data.mail.map(message => [`${message.at}:${message.seq}:${message.from}:${message.to}`, message]));
    for (const message of fleet.mail) mail.set(`${message.at}:${message.seq}:${message.from}:${message.to}`, message);
    data.mail = [...mail.values()].sort((a, b) => a.at - b.at).slice(-500);
  }
  function parentUsage(): RecordUsage {
    const file = session().sessionFile;
    if (!file) return { input: 0, output: 0 };
    let text: string, key: string;
    try {
      const stat = statSync(file);
      key = `${file}:${stat.ino}:${stat.size}:${stat.mtimeMs}`;
      if (cachedParent?.key === key) return cachedParent.usage;
      text = readFileSync(file, "utf8");
    } catch { return { input: 0, output: 0 }; }
    const usage: RecordUsage[] = [];
    for (const line of text.split("\n")) {
      if (!line) continue;
      try {
        const entry = JSON.parse(line) as { type?: string; usage?: TranscriptUsage; message?: { role?: string; usage?: TranscriptUsage } };
        const value = entry.type === "message" && ["assistant", "toolResult"].includes(entry.message?.role ?? "") ? entry.message?.usage
          : ["usage", "branch_summary", "compaction"].includes(entry.type ?? "") ? entry.usage : undefined;
        if (value) usage.push({ input: value.input ?? 0, output: value.output ?? 0,
          ...(value.cost?.total === undefined ? {} : { cost: value.cost.total }) });
      } catch { /* The final transcript line may still be streaming. */ }
    }
    const total = sum(usage);
    cachedParent = { key, usage: total };
    return total;
  }
  function record(): BuildRecord {
    captureFleet();
    const current = session();
    const parent = parentUsage();
    const children = sum(data.children.filter(child => child.kind !== "review").map(child => child.usage));
    const reviewers = sum(data.children.filter(child => child.kind === "review").map(child => child.usage));
    return { version: 1, id: current.id, kind: "build", project: current.project,
      title: Array.from(current.title?.trim() || data.title?.trim() || data.intent.map(entry => requestTitle(entry.text)).find(Boolean) || "Build session").slice(0, 120).join(""),
      intent: data.intent.slice(0, 32).map(prompt => ({ ...prompt, text: prompt.text.slice(0, 4096) })),
      created: current.created, updated: data.updated, repo_root: current.repoRoot, worktree: current.worktree,
      branch: data.branch === undefined ? current.branch : data.branch, base: current.base, outcome: data.outcome, diff: data.diff,
      commits: data.commits.slice(0, 200),
      slices: data.children.map(child => ({ id: child.id, title: child.title, kind: child.kind, owns: child.owns,
        model: child.model, effort: child.effort, state: child.state, started: child.startedAt === undefined ? undefined : iso(child.startedAt),
        ended: child.endedAt === undefined ? undefined : iso(child.endedAt), usage: { input: child.usage.input, output: child.usage.output, turns: child.usage.turns,
          ...(child.usage.cost === undefined ? {} : { cost: child.usage.cost }) },
        outside: [...new Set(child.edits.filter(edit => edit.outside).map(edit => edit.path))], report: child.report?.slice(0, 8192) })),
      verification: data.verification.map(({ attempt, head, result, failed_command, started, ended, log }) =>
        ({ attempt, head, result, failed_command, started, ended, log })),
      reviews: data.reviews, messages: data.mail.slice(-500).map(message => ({ at: iso(message.at), from: message.from, to: message.to,
        text: message.text.slice(0, 2048), delivery: message.delivery })), decisions: data.decisions,
      usage: { parent, children, reviewers, total: sum([parent, children, reviewers]) }, timeline: data.timeline,
      transcripts: { parent: current.sessionFile, children: Object.fromEntries(data.children.filter(child => child.sessionFile).map(child => [child.id, child.sessionFile!])) } };
  }
  async function readGit(current: BuildSessionRecord, git: Run, limit: number) {
    const summary = await summarize(git, current, options.fleet());
    const diff = { commits: summary.commits, files: summary.files, added: summary.added, removed: summary.removed };
    const text = current.base && summary.commits > 0
      ? await checked(git, ["log", `-${limit}`, "--format=%H%x00%s%x00%b%x00", `${current.base}..${summary.head}`], current.worktree) : "";
    const fields = text.split("\0");
    const ids = new Set(data.children.map(child => child.id));
    const commits: BuildRecord["commits"][number][] = [];
    for (let index = 0; index + 2 < fields.length && commits.length < limit; index += 3) {
      const sha = fields[index]!.trim(), subject = fields[index + 1]!;
      if (!sha) continue;
      const body = fields[index + 2]!;
      const explicit = /(?:^|\n)(?:Build-)?Slice:\s*([a-z][a-z0-9-]*)\s*(?:\n|$)/i.exec(body)?.[1];
      const prefix = /^\[([a-z][a-z0-9-]*)\](?:\s|$)/.exec(subject)?.[1];
      const slice = explicit ?? prefix;
      commits.push({ sha, subject, ...(slice && ids.has(slice) ? { slice } : {}) });
    }
    return { branch: summary.branch, diff, commits };
  }
  async function refresh() {
    captureFleet();
    const current = session();
    if (current.bare && current.repository === false) { persist(); return; }
    Object.assign(data, await readGit(current, options.git, 200));
    persist();
  }
  async function liveRecord(): Promise<BuildRecord> {
    if (data.outcome.state !== "open") return record();
    const current = session();
    const deadline = performance.now() + 1500;
    // The socket read must finish before the daemon's two-second deadline. Do not persist
    // this snapshot or emit an event that would cause subscribers to reread it in a loop.
    const git: Run = (argv, opts) => {
      const timeoutMs = Math.floor(deadline - performance.now());
      if (timeoutMs <= 0) throw new Error("Build record Git read timed out");
      return options.git(argv, { ...opts, timeoutMs });
    };
    const snapshot = current.base && current.repository !== false
      ? await readGit(current, git, 50).catch(() => undefined) : undefined;
    const latest = record();
    if (latest.outcome.state !== "open") return latest;
    return snapshot ? { ...latest, ...snapshot } : { ...latest, diff: undefined, commits: [] };
  }
  return {
    record, liveRecord, refresh,
    capture() { captureFleet(); persist(); },
    noteOwnerInput(text: string) {
      data.intent.push({ at: iso(now()), text });
      if (!data.title?.trim() && !session().title?.trim()) data.title = requestTitle(text);
      persist();
    },
    prompts: () => data.intent.map(prompt => ({ ...prompt })),
    finish(title: string, summary?: string) {
      if (!title.trim() || title.length > 120) throw new Error("A real build title of 1 to 120 characters is required.");
      data.title = title; data.summary = summary; persist();
    },
    gate(result) { data.verification.push(result); data.timeline.push({ at: result.ended, phase: "verify", detail: `${result.attempt}: ${result.result}` }); persist(); },
    latestGate: () => data.verification.at(-1),
    passedGate: (head, commands) => data.verification.findLast(gate => gate.result === "pass" && gate.head === head
      && gate.commands?.length === commands.length && gate.commands.every((command, index) => command === commands[index])),
    latestReview: () => data.reviews.at(-1),
    review(child: ChildSnapshot, head: string): RecordReview {
      if (!child.verdict) throw new Error(`Reviewer ${child.id} ended without a verdict${child.error ? `: ${child.error}` : "."}`);
      const verdict = child.verdict;
      const round: RecordReview = { round: data.reviews.length + 1, reviewer: child.id, model: child.model, head: verdict.head ?? head,
        verdict: verdict.verdict, at: iso(verdict.at), summary: verdict.summary,
        findings: verdict.findings.map((finding, index) => ({ ...finding, id: `F${index + 1}` })) };
      data.reviews.push(round); data.timeline.push({ at: round.at, phase: "review", detail: `${child.id}: ${round.verdict}` }); captureFleet(); persist(); return round;
    },
    resolve(round: number, finding: string, resolution: { state: "fixed" | "declined"; commit?: string; reason?: string }) {
      if (resolution.state === "declined" && !resolution.reason?.trim()) throw new Error("Declining a finding requires a reason.");
      const review = data.reviews.find(value => value.round === round);
      if (!review?.findings.some(value => value.id === finding)) throw new Error(`Unknown finding ${finding} in review round ${round}.`);
      data.reviews = data.reviews.map(value => value !== review ? value : { ...value, findings: value.findings.map(value => value.id === finding ? { ...value, resolution } : value) }); persist();
    },
    decision(kind: RecordDecision["kind"], text: string, answer?: string) { data.decisions.push({ at: iso(now()), kind, text, ...(answer === undefined ? {} : { answer }) }); persist(); },
    outcome(value: BuildRecord["outcome"]) { data.outcome = value; persist(); },
    timeline(phase: BuildRecord["timeline"][number]["phase"], detail: string) { data.timeline.push({ at: iso(now()), phase, detail }); persist(); },
  };
}
