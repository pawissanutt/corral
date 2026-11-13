import { resolve, basename } from "node:path";
import { realpath } from "node:fs/promises";
import type { Inspection } from "./inspection.ts";
import { parseChangedPaths, readCandidateDiff } from "./candidate-git.ts";
import { gitFailure, panelGit, repositoryRoot, parseStatus, parseNumstat, untrackedStats, comparisonArgs, relevantFile, readPanelDiff, FILE_LIMIT, type Comparison } from "./panels-git.ts";
import type { PanelFile } from "./panels-view.ts";
export const COMPARISONS: readonly Comparison[] = ["candidate", "session", "all", "unstaged", "staged"];
/** The HEAD each repository root had when this session first read it; persisted so resume keeps it. */
export interface ReviewBases { get(root: string): string | undefined; set(root: string, head: string): void }
export function memoryBases(): ReviewBases {
  const bases = new Map<string, string>();
  return { get: root => bases.get(root), set: (root, head) => { bases.set(root, head); } };
}
/** Reads the last persisted entry per root once, then appends only new roots. */
export function persistedBases(load: () => Iterable<unknown>, append: (data: { root: string; head: string }) => void): ReviewBases {
  let bases: Map<string, string> | undefined;
  const all = () => {
    if (bases) return bases;
    bases = new Map();
    for (const data of load()) {
      const { root, head } = (data ?? {}) as { root?: unknown; head?: unknown };
      if (typeof root === "string" && typeof head === "string" && /^[a-f0-9]{40,64}$/.test(head)) bases.set(root, head);
    }
    return bases;
  };
  return { get: root => all().get(root), set: (root, head) => { all().set(root, head); append({ root, head }); } };
}
export function comparisonLabel(value: Comparison): string {
  switch (value) {
    case "candidate": return "Committed candidate · comparison base → HEAD";
    case "session": return "Since session start · recorded base → working tree";
    case "all": return "All uncommitted · HEAD → working tree";
    case "unstaged": return "Unstaged · index → working tree";
    case "staged": return "Staged · HEAD → index";
  }
}
export function fileStatus(file: PanelFile): string {
  if (file.status.includes("U") || ["AA", "DD"].includes(file.status)) return "Conflict";
  if (file.status === "??") return "New";
  if (file.status.includes("R")) return "Renamed";
  if (file.status.includes("D")) return "Deleted";
  if (file.status.includes("A")) return "Added";
  return "Modified";
}
/** Base-relative changes retain live conflicts and untracked files, with one row per path. */
export function sessionFiles(changed: readonly PanelFile[], worktree: readonly PanelFile[]): PanelFile[] {
  const files = new Map(changed.map(file => [file.path, file]));
  for (const file of worktree) {
    if (file.status === "??" || fileStatus(file) === "Conflict") files.set(file.path, file);
  }
  return [...files.values()].slice(0, FILE_LIMIT);
}
export interface FileStat { readonly added: number | null; readonly removed: number | null }
export interface PatchSnapshot { readonly root: string; readonly path: string; readonly comparison: Comparison; readonly head: string | undefined; readonly base?: string; readonly at: number; readonly text: string; readonly file: PanelFile; readonly stat: FileStat | undefined }
export class ReviewStore {
  patchSnapshot: PatchSnapshot | undefined;
  candidateBase: string | undefined;
  /** The base the last session read compared against; rewritten when the recorded one left HEAD's history. */
  sessionBase: string | undefined; baseRewritten = false;
  root: string; branch = "unknown"; head: string | undefined; patchPath: string | undefined; patchComparison: Comparison = "session"; patchAt: number | undefined; comparison: Comparison = "session"; query = "";
  files: readonly PanelFile[] = []; selected: string | undefined;
  /** Working-tree status at the last read (also in candidate mode), for the other-comparison counts. */
  worktree: readonly PanelFile[] | undefined;
  patch = "Select a file to inspect its diff."; error: string | undefined;
  checkedAt: number | undefined; refreshing = false; revision = 0;
  stats: ReadonlyMap<string, FileStat> = new Map();
  private epoch = 0; private patchEpoch = 0; private disposed = false;
  private explicitScope = false;
  get comparisons(): readonly Comparison[] { return this.explicitScope || !this.candidate ? COMPARISONS.filter(value => value !== "candidate") : COMPARISONS; }
  private readonly abort = new AbortController();
  private active: Promise<void> | undefined;
  constructor(cwd: string, private readonly changed: () => void, private readonly candidate?: () => Promise<Inspection>, public scopeRequired = false, private strictRoot = false, private readonly bases: ReviewBases = memoryBases()) {
    this.root = resolve(cwd); if (candidate) this.comparison = "candidate";
  }
  get name(): string { return basename(this.root); }
  get visible(): readonly PanelFile[] {
    const words = this.query.toLowerCase().split(/\s+/).filter(Boolean);
    return this.files.filter(file => relevantFile(file, this.comparison) && words.every(word => file.path.toLowerCase().includes(word)));
  }
  get current(): PanelFile | undefined { return this.visible.find(file => file.path === this.selected); }
  async setRepository(path: string): Promise<void> {
    this.explicitScope = true; this.strictRoot = true;
    this.epoch++; this.patchEpoch++; this.root = resolve(this.root, path); this.scopeRequired = false; this.comparison = "session"; this.candidateBase = undefined; this.sessionBase = undefined; this.baseRewritten = false;
    this.head = undefined; this.branch = "unknown"; this.error = undefined; this.stats = new Map(); this.query = ""; this.patchSnapshot = undefined;
    this.files = []; this.worktree = undefined; this.selected = undefined; this.patchPath = undefined; this.checkedAt = undefined; this.patch = "Loading repository…";
    this.changed();
    // Wait for an old read before refreshing the newly selected scope.
    await this.active; await this.refresh();
  }
  async setComparison(value: Comparison): Promise<void> {
    if (!this.comparisons.includes(value)) { this.error = "Candidate comparison belongs to the bound task, not this Files scope."; this.changed(); return; }
    this.comparison = value; this.selected = undefined; this.epoch++; this.patchEpoch++;
    this.patchSnapshot = undefined; this.patchPath = undefined; this.error = undefined;
    await this.active; await this.refresh();
  }
  refresh(): Promise<void> {
    if (this.disposed || this.scopeRequired) return Promise.resolve();
    if (this.active) return this.active;
    const operation = this.read(); this.active = operation;
    void operation.finally(() => { if (this.active === operation) this.active = undefined; });
    return operation;
  }
  private async read(): Promise<void> {
    const epoch = this.epoch, cwd = this.root, comparison = this.comparison;
    this.refreshing = true; this.changed();
    try {
      if (comparison === "candidate") { await this.readCandidate(epoch); return; }
      const root = await repositoryRoot(cwd, this.abort.signal);
      if (this.strictRoot && await realpath(cwd) !== await realpath(root)) {
        throw new Error("Choose a project repository root or worktree; parent workspace changes are not shown.");
      }
      const [status, branch, head] = await Promise.all([
        panelGit(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"], this.abort.signal),
        panelGit(root, ["rev-parse", "--abbrev-ref", "HEAD"], this.abort.signal).catch(() => "unborn"),
        panelGit(root, ["rev-parse", "--verify", "HEAD"], this.abort.signal).catch(() => ""),
      ]);
      const worktree = parseStatus(status), current = head.trim() || undefined;
      let files = worktree, statText: string, sessionBase: string | undefined, rewritten = false;
      if (comparison === "session" && current) {
        // Committed work stays visible: the recorded base against the working tree, plus untracked files.
        ({ base: sessionBase, rewritten } = await this.sessionStart(root, current));
        const [paths, stats] = await Promise.all([
          panelGit(root, ["diff", "--no-ext-diff", "--no-textconv", "--find-renames", "--name-status", "-z", sessionBase], this.abort.signal),
          panelGit(root, ["diff", "--no-ext-diff", "--no-textconv", "--find-renames", "--numstat", "-z", sessionBase], this.abort.signal).catch(() => ""),
        ]);
        files = sessionFiles(parseChangedPaths(paths), worktree);
        statText = stats;
      } else statText = await panelGit(root, ["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z", ...comparisonArgs(comparison === "session" ? "all" : comparison)], this.abort.signal).catch(() => "");
      // Numstat leaves untracked files out; their added lines are counted from the files themselves, numstat winning.
      const untracked = await untrackedStats(root, files, this.abort.signal);
      if (this.disposed || epoch !== this.epoch) return;
      this.sessionBase = sessionBase; this.baseRewritten = rewritten;
      this.root = root; this.head = current; this.branch = branch.trim(); this.files = files; this.worktree = worktree;
      this.stats = new Map([...untracked, ...parseNumstat(statText)]);
      this.error = undefined; this.checkedAt = Date.now(); this.revision++;
      if (this.selected === undefined) this.selected = this.visible[0]?.path;
      await this.loadSelected();
    } catch (error) {
      if (!this.disposed && epoch === this.epoch) this.error = gitFailure(error);
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  private async sessionStart(root: string, head: string): Promise<{ base: string; rewritten: boolean }> {
    const recorded = this.bases.get(root);
    if (!recorded) { this.bases.set(root, head); return { base: head, rewritten: false }; }
    if (recorded === head) return { base: head, rewritten: false };
    const signal = this.abort.signal;
    if (await panelGit(root, ["merge-base", "--is-ancestor", recorded, head], signal).then(() => true, () => false)) return { base: recorded, rewritten: false };
    // Rebased or reset away: compare from where the histories meet, and say so.
    const fork = (await panelGit(root, ["merge-base", recorded, head], signal).catch(() => "")).trim();
    return { base: /^[a-f0-9]{40,64}$/.test(fork) ? fork : head, rewritten: true };
  }
  private async readCandidate(epoch: number): Promise<void> {
    if (!this.candidate) throw new Error("No task/run candidate bound to this session; c selects working changes");
    const data = await this.candidate();
    const root = await repositoryRoot(data.root, this.abort.signal);
    const [paths, stats, status] = await Promise.all([
      panelGit(root, ["diff", "--no-ext-diff", "--no-textconv", "--find-renames", "--name-status", "-z", data.base, data.head], this.abort.signal),
      panelGit(root, ["diff", "--no-ext-diff", "--no-textconv", "--find-renames", "--numstat", "-z", data.base, data.head], this.abort.signal),
      panelGit(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"], this.abort.signal).then(parseStatus, () => undefined),
    ]);
    if (this.disposed || epoch !== this.epoch) return;
    this.root = root; this.head = data.head; this.candidateBase = data.base; this.branch = data.task + "/" + data.run;
    this.files = parseChangedPaths(paths); this.worktree = status; this.stats = parseNumstat(stats); this.error = undefined;
    this.checkedAt = Date.now(); this.revision++;
    if (this.selected === undefined) this.selected = this.visible[0]?.path;
    await this.loadSelected();
  }
  async select(path: string): Promise<void> { this.selected = path; this.changed(); await this.loadSelected(); }
  private async loadSelected(): Promise<void> {
    const file = this.current, epoch = ++this.patchEpoch, root = this.root, comparison = this.comparison;
    const head = this.head;
    if (!file) { this.patchSnapshot = undefined; this.patchPath = this.selected; this.patchComparison = comparison; this.patchAt = Date.now(); this.patch = this.error ?? (this.selected ? "Selected path is no longer in this comparison. Choose another file with j/k." : this.files.length ? "No files match the current filter." : comparison === "candidate" ? "No committed candidate changes." : "Working tree clean."); this.changed(); return; }
    try {
      const base = comparison === "session" ? this.sessionBase : this.candidateBase, stat = this.stats.get(file.path);
      const metadata = { file: { ...file }, stat: stat ? { ...stat } : undefined };
      const patch = comparison === "candidate" ? await readCandidateDiff(root, file, base, head, this.abort.signal)
        : await readPanelDiff(root, file, this.abort.signal, comparison, comparison === "session" ? base : head);
      if (!this.disposed && epoch === this.patchEpoch && root === this.root && comparison === this.comparison && file.path === this.selected) {
        this.patch = patch; this.patchComparison = comparison; this.patchPath = file.path; this.patchAt = Date.now();
        this.patchSnapshot = { ...metadata, root, path: file.path, comparison, head, ...((comparison === "candidate" || comparison === "session") && base ? { base } : {}), at: this.patchAt, text: patch }; this.changed();
      }
    } catch (error) {
      if (!this.disposed && epoch === this.patchEpoch && root === this.root && comparison === this.comparison && file.path === this.selected) { this.patchSnapshot = undefined; this.patchPath = file.path; this.patchComparison = comparison; this.patchAt = Date.now(); this.patch = "Diff unavailable: " + gitFailure(error); this.changed(); }
    }
  }
  get limitNotice(): string { return this.files.length === FILE_LIMIT ? `First ${FILE_LIMIT} paths; narrow repository scope to inspect more.` : ""; }
  dispose(): void { this.disposed = true; this.abort.abort(); }
}
