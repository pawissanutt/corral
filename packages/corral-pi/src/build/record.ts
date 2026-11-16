import { randomBytes } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checked, json, type Run } from "./corral-cli.ts";
import { summarize } from "./git.ts";
import type { BuildContext, BuildSessionRecord, ChildSnapshot, FleetSnapshot } from "./types.ts";
import type { BuildRecord, GateReceipt, RecordDecision, RecordPublished, RecordReview, RecordUsage } from "./record-types.ts";

interface JournalData {
  version: 1;
  title?: string;
  summary?: string;
  intent: { at: string; text: string }[];
  ownerSteers: Record<string, number>;
  verification: GateReceipt[];
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
  refresh(): Promise<void>;
  publish(): Promise<string | undefined>;
  capture(): void;
  noteOwnerInput(text: string): void;
  prompts(): { at: string; text: string }[];
  finish(title: string, summary?: string): void;
  gate(receipt: GateReceipt): void;
  latestGate(): GateReceipt | undefined;
  latestReview(): RecordReview | undefined;
  review(child: ChildSnapshot, head: string): RecordReview;
  resolve(round: number, finding: string, resolution: { state: "fixed" | "declined"; commit?: string; reason?: string }): void;
  decision(kind: RecordDecision["kind"], text: string, answer?: string): void;
  outcome(value: BuildRecord["outcome"]): void;
  timeline(phase: BuildRecord["timeline"][number]["phase"], detail: string): void;
}
const iso = (at: number) => new Date(at).toISOString();
function sum(values: readonly RecordUsage[]): RecordUsage {
  return { input: values.reduce((n, v) => n + v.input, 0), output: values.reduce((n, v) => n + v.output, 0),
    ...(values.every(v => v.cost !== undefined) ? { cost: values.reduce((n, v) => n + v.cost!, 0) } : {}) };
}

export function createJournal(options: {
  context: BuildContext; run: Run; git: Run; fleet: () => FleetSnapshot;
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
      title: (data.title ?? data.intent[0]?.text.split(/\r?\n/)[0] ?? "Build session").slice(0, 120),
      intent: data.intent.slice(0, 32).map(prompt => ({ ...prompt, text: prompt.text.slice(0, 4096) })),
      created: current.created, updated: data.updated, repo_root: current.repoRoot, worktree: current.worktree,
      branch: data.branch === undefined ? current.branch : data.branch, base: current.base, outcome: data.outcome, diff: data.diff,
      commits: data.commits.slice(0, 200),
      slices: data.children.map(child => ({ id: child.id, title: child.title, kind: child.kind, owns: child.owns,
        model: child.model, effort: child.effort, state: child.state, started: child.startedAt === undefined ? undefined : iso(child.startedAt),
        ended: child.endedAt === undefined ? undefined : iso(child.endedAt), usage: { input: child.usage.input, output: child.usage.output, turns: child.usage.turns,
          ...(child.usage.cost === undefined ? {} : { cost: child.usage.cost }) },
        outside: [...new Set(child.edits.filter(edit => edit.outside).map(edit => edit.path))], report: child.report?.slice(0, 8192) })),
      verification: data.verification.map(({ commands: _commands, receipt: _receipt, ...attempt }) => attempt),
      reviews: data.reviews, messages: data.mail.slice(-500).map(message => ({ at: iso(message.at), from: message.from, to: message.to,
        text: message.text.slice(0, 2048), delivery: message.delivery })), decisions: data.decisions,
      usage: { parent, children, reviewers, total: sum([parent, children, reviewers]) }, timeline: data.timeline,
      transcripts: { parent: current.sessionFile, children: Object.fromEntries(data.children.filter(child => child.sessionFile).map(child => [child.id, child.sessionFile!])) } };
  }
  async function refresh() {
    captureFleet();
    const current = session();
    if (current.bare && current.repository === false) { persist(); return; }
    const summary = await summarize(options.git, current, options.fleet());
    data.branch = summary.branch;
    data.diff = { commits: summary.commits, files: summary.files, added: summary.added, removed: summary.removed };
    const text = current.base ? await checked(options.git, ["log", "-200", "--format=%H%x00%s%x00%b%x00", `${current.base}..HEAD`], current.worktree) : "";
    const fields = text.split("\0");
    const ids = new Set(data.children.map(child => child.id));
    const commits: BuildRecord["commits"][number][] = [];
    for (let index = 0; index + 2 < fields.length && commits.length < 200; index += 3) {
      const sha = fields[index]!.trim(), subject = fields[index + 1]!;
      if (!sha) continue;
      const body = fields[index + 2]!;
      const explicit = /(?:^|\n)(?:Build-)?Slice:\s*([a-z][a-z0-9-]*)\s*(?:\n|$)/i.exec(body)?.[1];
      const prefix = /^\[([a-z][a-z0-9-]*)\](?:\s|$)/.exec(subject)?.[1];
      const slice = explicit ?? prefix;
      commits.push({ sha, subject, ...(slice && ids.has(slice) ? { slice } : {}) });
    }
    data.commits = commits;
    persist();
  }
  async function publish(): Promise<string | undefined> {
    if (session().bare) return undefined;
    const temporary = join(context.dir, `.record-${randomBytes(12).toString("hex")}.json`);
    try {
      captureFleet(); persist();
      const current = session();
      if (!current.workspaceRoot || !current.project) return undefined;
      writeFileSync(temporary, `${JSON.stringify(record())}\n`, { flag: "wx", mode: 0o600 });
      await json<RecordPublished>(options.run, ["build", "record", "--file", temporary, "--json"], current.workspaceRoot);
      return undefined;
    } catch (error) { return `Build record publication failed: ${error instanceof Error ? error.message : String(error)}`; }
    finally { try { rmSync(temporary, { force: true }); } catch { /* Publication cleanup never changes the owner's exit. */ } }
  }
  return {
    record, refresh, publish,
    capture() { captureFleet(); persist(); },
    noteOwnerInput(text: string) { data.intent.push({ at: iso(now()), text }); persist(); },
    prompts: () => data.intent.map(prompt => ({ ...prompt })),
    finish(title: string, summary?: string) {
      if (!title.trim() || title.length > 120) throw new Error("A real build title of 1 to 120 characters is required.");
      data.title = title; data.summary = summary; persist();
    },
    gate(receipt: GateReceipt) { data.verification.push(receipt); data.timeline.push({ at: receipt.ended, phase: "verify", detail: `${receipt.attempt}: ${receipt.result}` }); persist(); },
    latestGate: () => data.verification.at(-1),
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
