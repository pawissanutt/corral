import { realpath } from "node:fs/promises";
import { basename, isAbsolute, resolve } from "node:path";
import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
import type { ChildSnapshot } from "../../build/types.ts";
import { utf8Prefix } from "../../roles/assistant/owner-action.ts";
import { parseChangedPaths, readCandidateDiff } from "../../ui/candidate-git.ts";
import { comparisonArgs, FILE_LIMIT, gitFailure, panelGit, parseNumstat, parseStatus, readPanelDiff,
  repositoryRoot, untrackedStats, type Comparison } from "../../ui/panels-git.ts";
import type { PanelFile } from "../../ui/panels-view.ts";
import { comparisonLabel, COMPARISONS, fileStatus, type Candidate, type ReviewBases } from "../../ui/review-store.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, cursorOffset, fingerprintPart, fitRows, jsonBytes, pageRows, parseCursor, partCursor } from "./state.ts";

const GIT_READ_BYTES = 256 * 1024;
const compare = ["--no-ext-diff", "--no-textconv", "--find-renames"];
const gitId = /^[a-f0-9]{40,64}$/;
export interface ChangedFile { path: string; original_path?: string; status: string; added?: number; removed?: number; authors: string[]; outside: boolean }
interface ComparisonInfo { id: Comparison; label: string; unavailable?: string }
interface Scope { id: string; label: string; path: string }
interface ChangeQuery { comparison: Comparison; scope?: string }
interface ChangesRead extends ChangeQuery {
  comparisons: ComparisonInfo[]; scopes: Pick<Scope, "id" | "label">[]; root: string; base?: string; head?: string;
  files: ChangedFile[]; total: number; added: number; removed: number; truncated: boolean; error?: string;
}
interface PatchRead { path: string; patch: string; truncated: boolean }
export interface ChangesSource {
  cwd: string; defaultComparison: Comparison; bases: ReviewBases; scopeRequired?: boolean;
  children(): readonly ChildSnapshot[]; projects?: () => Promise<unknown>; candidate?: () => Promise<Candidate>;
  changed(): void;
}

/** A full list row carries the path unchanged: the same path keys the patch operation. */
export function changesPage(read: ChangesRead, after?: string) {
  const meta = { ...read, root: clipText(read.root, 4096), files: [] };
  let truncated = read.truncated || read.files.length > FILE_LIMIT;
  const overhead = jsonBytes(meta), budget = SESSION_PART_MAX - overhead - 65;
  const files = read.files.slice(0, FILE_LIMIT).map(file => {
    if (jsonBytes(file) <= budget) return file;
    truncated = true;
    // Keep the exact path addressable, even when two heavily escaped paths cannot share a row.
    return { ...file, original_path: undefined };
  });
  const { items, next } = pageRows(files, after, fingerprintPart(["changes", read]), 200, overhead);
  return { ...meta, files: items, truncated, next };
}

function patchPage(read: PatchRead, query: ChangeQuery, after?: string) {
  const { patch: text, ...meta } = read;
  const key = fingerprintPart(["change_file", query, read]);
  const offset = after === undefined ? 0 : cursorOffset(after, key);
  if (offset > text.length || /[\uDC00-\uDFFF]/.test(text[offset] ?? "")) throw new BridgeRefusal("bad_input", "patch cursor is outside the text");
  const patch = clipText(text.slice(offset), SESSION_PART_MAX - jsonBytes({ ...meta, patch: "" }) - 64);
  const end = offset + patch.length;
  return { ...meta, patch, next: end < text.length ? partCursor(key, end) : undefined };
}

function gitOverflow(error: unknown): { stdout?: unknown } | undefined {
  return error instanceof Error && "code" in error && error.code === "ERR_CHILD_PROCESS_STDIO_MAXBUFFER"
    ? error as Error & { stdout?: unknown } : undefined;
}

export async function boundedPatch(path: string, read: () => Promise<string>): Promise<PatchRead> {
  try {
    const raw = await read(), patch = utf8Prefix(raw, GIT_READ_BYTES);
    return { path, patch, truncated: patch !== raw };
  }
  catch (error) {
    const overflow = gitOverflow(error);
    if (!overflow) throw new BridgeRefusal("patch_unavailable", clipText(gitFailure(error), 512));
    const prefix = typeof overflow.stdout === "string" ? utf8Prefix(overflow.stdout, GIT_READ_BYTES) : "";
    return { path, patch: prefix.replace(/\uFFFD$/u, ""), truncated: true };
  }
}

async function gitRows(read: Promise<string>) {
  try { return { text: await read, truncated: false }; }
  catch (error) {
    const overflow = gitOverflow(error);
    if (!overflow) throw error;
    const text = typeof overflow.stdout === "string" ? overflow.stdout : "";
    return { text: text.slice(0, text.lastIndexOf("\0") + 1), truncated: true };
  }
}

export function boundCandidate(raw: unknown, task: string, run: string): Candidate {
  const data = raw as { task?: { id?: unknown }; runs?: { id?: unknown; worktree?: unknown; base_sha?: unknown; head_sha?: unknown }[] } | undefined;
  const row = data?.task?.id === task && Array.isArray(data.runs) ? data.runs.find(row => row?.id === run) : undefined;
  if (!row || typeof row.worktree !== "string" || !isAbsolute(row.worktree) || typeof row.base_sha !== "string" || !gitId.test(row.base_sha) ||
      row.head_sha != null && (typeof row.head_sha !== "string" || !gitId.test(row.head_sha))) throw new Error("No captured candidate for the bound run");
  return { task, run, root: row.worktree, base: row.base_sha, head: typeof row.head_sha === "string" ? row.head_sha : undefined };
}

function projectScopes(cwd: string, raw: unknown, includeBound: boolean): Scope[] {
  const data = raw as { workspace?: unknown; projects?: Record<string, { path?: unknown }> } | undefined;
  const scopes: Scope[] = includeBound ? [{ id: "bound", label: clipText(basename(cwd) || cwd, 160), path: cwd }] : [];
  if (typeof data?.workspace === "string" && isAbsolute(data.workspace) && data.projects && typeof data.projects === "object") {
    for (const [label, project] of Object.entries(data.projects)) {
      if (typeof project?.path !== "string") continue;
      const path = resolve(data.workspace, project.path);
      if (!includeBound || path !== cwd) scopes.push({ id: fingerprintPart([label, path]), label: clipText(label, 160), path });
      if (scopes.length === 64) break;
    }
  }
  return scopes;
}

function parseQuery({ comparison, scope }: Record<string, unknown>): ChangeQuery {
  if (typeof comparison !== "string" || !COMPARISONS.some(id => id === comparison) ||
      scope !== undefined && (typeof scope !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(scope))) {
    throw new BridgeRefusal("bad_input", "invalid changes comparison or scope");
  }
  return { comparison: comparison as Comparison, scope };
}

function attribute(children: readonly ChildSnapshot[]) {
  const edits = new Map<string, { authors: string[]; outside: boolean }>();
  for (const child of children) for (const edit of child.edits) {
    let row = edits.get(edit.path);
    if (!row) edits.set(edit.path, row = { authors: [], outside: false });
    if (row.authors.at(-1) !== child.id) row.authors.push(child.id);
    row.outside ||= edit.outside;
  }
  return edits;
}

interface Observation { value: string; read(): Promise<unknown> }
export class RemoteChanges {
  private readonly abort = new AbortController();
  private readonly observed = new Map<string, Observation>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private index = 0;
  version = 0;
  count: number | undefined;
  private checkingCandidate: { until: number; read: Promise<{ value?: Candidate; error?: string }> } | undefined;

  constructor(private readonly source: ChangesSource) {}

  async captureBase(root: string | undefined): Promise<void> {
    if (!root || this.source.scopeRequired || this.source.bases.get(root)) return;
    const head = (await panelGit(root, ["rev-parse", "--verify", "HEAD"], this.abort.signal).catch(() => "")).trim();
    if (gitId.test(head) && !this.source.bases.get(root)) this.source.bases.set(root, head);
  }

  register(registry: RemoteRegistry): void {
    registry.register({ name: "changes", parse: args => ({ query: parseQuery(args), after: parseCursor(args.after) }),
      run: async ({ query, after }) => changesPage(await this.observe(["changes", query], () => this.read(query)), after) });
    registry.register({ name: "change_file", parse: args => {
      const { path } = args;
      if (typeof path !== "string" || !path || path.includes("\0") || Buffer.byteLength(path) > 4096) throw new BridgeRefusal("bad_input", "invalid change path");
      return { query: parseQuery(args), path, after: parseCursor(args.after) };
    }, run: async ({ query, path, after }) => patchPage(await this.observe(["patch", query, path], () => this.readPatch(query, path)), query, after) });
  }

  close(): void {
    this.abort.abort();
    clearTimeout(this.timer);
    this.observed.clear();
  }

  private async observe<T>(identity: unknown, read: () => Promise<T>): Promise<T> {
    const key = fingerprintPart(identity), result = await read();
    if (this.abort.signal.aborted) return result;
    const prior = this.observed.get(key), value = fingerprintPart(result);
    this.observed.delete(key);
    if (this.observed.size >= 8) this.observed.delete(this.observed.keys().next().value!);
    this.observed.set(key, { value, read });
    if (!prior || prior.value !== value) { this.version++; this.source.changed(); }
    this.schedule();
    return result;
  }

  private schedule(): void {
    if (this.timer || this.abort.signal.aborted || !this.observed.size) return;
    this.timer = setTimeout(() => { void this.sample(); }, 1500);
    this.timer.unref();
  }

  private async sample(): Promise<void> {
    try {
      const entries = [...this.observed];
      if (!entries.length) return;
      const [key, observation] = entries[this.index++ % entries.length]!;
      const value = fingerprintPart(await observation.read().catch(() => null));
      if (this.observed.get(key) === observation && value !== observation.value) {
        observation.value = value;
        this.version++;
        this.source.changed();
      }
    } finally { this.timer = undefined; this.schedule(); }
  }

  private candidate() {
    const source = this.source.candidate;
    if (!source) return Promise.resolve({ value: undefined, error: "This session has no bound task candidate" });
    const now = Date.now();
    if (!this.checkingCandidate || this.checkingCandidate.until <= now) {
      this.checkingCandidate = { until: now + 5000, read: source().then(value => ({ value }),
        error => ({ error: clipText(error instanceof Error ? error.message : String(error), 256) })) };
    }
    return this.checkingCandidate.read;
  }

  private async read(query: ChangeQuery): Promise<ChangesRead> {
    const { comparison, scope } = query, signal = this.abort.signal;
    const [configured, candidate] = await Promise.all([this.source.projects?.().catch(() => undefined), this.candidate()]);
    const allowed = projectScopes(this.source.cwd, configured, !this.source.scopeRequired);
    const scopes = fitRows(allowed.map(({ id, label }) => ({ id, label })), 0, 64, 12 * 1024);
    const selected = allowed.find(entry => entry.id === (scope ?? "bound"));
    const candidateReason = scope && scope !== "bound" ? "Candidate belongs to the bound worktree" : candidate.error;
    const comparisons: ComparisonInfo[] = COMPARISONS.map(id => ({ id, label: comparisonLabel(id), unavailable: id === "candidate" ? candidateReason : undefined }));
    const result: ChangesRead = { comparison, scope, comparisons, scopes, root: selected?.path ?? this.source.cwd,
      files: [], total: 0, added: 0, removed: 0, truncated: false };
    try {
      if (!scope && this.source.scopeRequired) {
        for (const item of comparisons) item.unavailable ??= "Choose a project repository";
        throw new Error("Choose a project repository");
      }
      if (!selected || !scopes.some(entry => entry.id === selected.id)) throw new Error("Unknown repository scope");
      if (comparison === "candidate" && (!candidate.value || candidateReason)) throw new Error(candidateReason ?? "Candidate unavailable");
      const cwd = comparison === "candidate" ? candidate.value!.root : selected.path;
      let root: string;
      try { root = await repositoryRoot(cwd, signal); }
      catch (error) {
        for (const item of comparisons) item.unavailable ??= clipText(gitFailure(error), 512);
        throw error;
      }
      if ((scope && scope !== "bound" || comparison === "candidate") && await realpath(cwd) !== await realpath(root)) {
        throw new Error("The selected scope is not a repository root");
      }
      result.root = root;
      const rev = async (ref: string) => (await panelGit(root, ["rev-parse", "--verify", ref], signal).catch(() => "")).trim();
      const head = (comparison === "candidate" ? candidate.value?.head : undefined) ?? (await rev("HEAD") || undefined);
      result.head = head;
      let args: string[];
      if (comparison === "candidate") {
        if (!head || !await rev(candidate.value!.base + "^{commit}")) throw new Error("Captured candidate commits are unavailable");
        result.base = candidate.value!.base;
        args = [result.base, head];
      } else if (comparison === "session") {
        let base = this.source.bases.get(root);
        if (!base && head) { this.source.bases.set(root, head); base = head; }
        if (base && base !== head && head) {
          const fork = (await panelGit(root, ["merge-base", base, head], signal).catch(() => "")).trim();
          if (gitId.test(fork)) base = fork;
          else if (!await rev(base + "^{commit}")) {
            comparisons.find(item => item.id === "session")!.unavailable = "The recorded comparison base no longer exists";
            throw new Error("The recorded comparison base no longer exists");
          }
        }
        result.base = base;
        args = base ? [base] : ["--cached"];
      } else {
        result.base = comparison === "unstaged" ? undefined : head;
        args = comparison === "all" && !head ? ["--cached"] : comparisonArgs(comparison);
      }
      const untracked = comparison !== "candidate" && comparison !== "staged";
      const [paths, stats, others, status] = await Promise.all([
        gitRows(panelGit(root, ["diff", ...compare, "--name-status", "-z", ...args], signal)),
        gitRows(panelGit(root, ["diff", ...compare, "--numstat", "-z", ...args], signal)),
        untracked ? gitRows(panelGit(root, ["ls-files", "--others", "--exclude-standard", "-z"], signal)) : undefined,
        comparison !== "candidate" ? gitRows(panelGit(root, ["status", "--porcelain=v1", "-z", "--untracked-files=no"], signal)) : undefined,
      ]);
      const counts = new Map(parseNumstat(stats.text));
      const changed = changedPaths(paths.text);
      const tracked = parseChangedPaths(paths.text.slice(0, changed.end)), conflicts = new Map((status ? parseStatus(status.text) : [])
        .filter(file => fileStatus(file) === "Conflict").map(file => [file.path, file]));
      const rows = new Map<string, PanelFile>();
      for (const file of tracked) rows.set(file.path, conflicts.get(file.path) ?? file);
      for (const [path, file] of conflicts) if (!rows.has(path) && rows.size < FILE_LIMIT) rows.set(path, file);
      const all = changed.paths;
      for (const path of conflicts.keys()) all.add(path);
      for (const path of others?.text.split("\0") ?? []) if (path && !rows.has(path)) {
        if (rows.size < FILE_LIMIT) rows.set(path, { path, status: "??" });
        all.add(path);
      }
      for (const [path, stat] of await untrackedStats(root, [...rows.values()], signal)) counts.set(path, stat);
      const edits = comparison === "candidate" || selected.id === "bound" ? attribute(this.source.children()) : undefined;
      for (const file of rows.values()) {
        const stats = counts.get(file.path), current = edits?.get(file.path), previous = file.originalPath ? edits?.get(file.originalPath) : undefined;
        result.files.push({ path: file.path, original_path: file.originalPath, status: file.status,
          added: stats?.added ?? undefined, removed: stats?.removed ?? undefined,
          authors: [...new Set<string>([...previous?.authors ?? [], ...current?.authors ?? []])].slice(0, 64), outside: !!(current?.outside || previous?.outside) });
      }
      for (const stat of counts.values()) { result.added += stat.added ?? 0; result.removed += stat.removed ?? 0; }
      result.total = all.size;
      result.truncated = result.total > FILE_LIMIT || paths.truncated || stats.truncated || !!others?.truncated || !!status?.truncated;
    } catch (error) {
      result.error = clipText(gitFailure(error), 512);
      comparisons.find(item => item.id === comparison)!.unavailable ??= result.error;
    }
    if (comparison === this.source.defaultComparison && (scope === undefined || scope === "bound")) {
      const count = result.error === undefined ? result.total : undefined;
      if (this.count !== count) { this.count = count; this.source.changed(); }
    }
    return result;
  }

  private async readPatch(query: ChangeQuery, path: string): Promise<PatchRead> {
    const { root, base, head, files, error } = await this.read(query);
    const file = files.find(row => row.path === path);
    if (!file) throw new BridgeRefusal("patch_unavailable", error ?? "path is not in this comparison");
    const selected = { path, status: file.status, originalPath: file.original_path };
    const signal = this.abort.signal;
    return boundedPatch(path, () => query.comparison === "candidate"
      ? readCandidateDiff(root, selected, base, head, signal)
      : readPanelDiff(root, selected, signal, query.comparison, query.comparison === "session" ? base : query.comparison === "all" ? head : undefined));
  }
}

function changedPaths(text: string) {
  const fields = text.split("\0"), paths = new Set<string>();
  let end = 0;
  for (let i = 0; fields[i];) {
    const count = /^[RC]/.test(fields[i]!) ? 3 : 2;
    if (i + count >= fields.length) break;
    paths.add(fields[i + count - 1]!);
    for (let j = 0; j < count; j++) end += fields[i++]!.length + 1;
  }
  return { paths, end };
}
