import { resolve, basename } from "node:path";
import { panelGit, repositoryRoot, parseStatus, parseNumstat, comparisonArgs, relevantFile, readPanelDiff, FILE_LIMIT, type Comparison } from "./panels-git.ts";
import type { PanelFile } from "./panels-view.ts";
export const COMPARISONS: readonly Comparison[] = ["all", "unstaged", "staged"];
export function comparisonLabel(value: Comparison): string {
  switch (value) {
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
export interface PatchSnapshot { readonly root: string; readonly path: string; readonly comparison: Comparison; readonly head: string | undefined; readonly at: number; readonly text: string }
export class ReviewStore {
  patchSnapshot: PatchSnapshot | undefined;
  root: string; branch = "unknown"; head: string | undefined; patchPath: string | undefined; patchComparison: Comparison = "all"; patchAt: number | undefined; comparison: Comparison = "all"; query = "";
  files: readonly PanelFile[] = []; selected: string | undefined;
  patch = "Select a file to inspect its diff."; error: string | undefined;
  checkedAt: number | undefined; refreshing = false; revision = 0;
  stats: ReadonlyMap<string, { added: number | null; removed: number | null }> = new Map();
  private epoch = 0; private patchEpoch = 0; private disposed = false;
  private readonly abort = new AbortController();
  private active: Promise<void> | undefined;
  constructor(cwd: string, private readonly changed: () => void) { this.root = resolve(cwd); }
  get name(): string { return basename(this.root); }
  get visible(): readonly PanelFile[] {
    const words = this.query.toLowerCase().split(/\s+/).filter(Boolean);
    return this.files.filter(file => relevantFile(file, this.comparison) && words.every(word => file.path.toLowerCase().includes(word)));
  }
  get current(): PanelFile | undefined { return this.visible.find(file => file.path === this.selected); }
  async setRepository(path: string): Promise<void> {
    this.epoch++; this.patchEpoch++; this.root = resolve(this.root, path);
    this.files = []; this.selected = undefined; this.patchPath = undefined; this.checkedAt = undefined; this.patch = "Loading repository…";
    // Wait for an old read before refreshing the newly selected scope.
    await this.active; await this.refresh();
  }
  async setComparison(value: Comparison): Promise<void> {
    this.comparison = value; this.selected = undefined; this.epoch++; await this.active; await this.refresh();
  }
  refresh(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.active) return this.active;
    const operation = this.read(); this.active = operation;
    void operation.finally(() => { if (this.active === operation) this.active = undefined; });
    return operation;
  }
  private async read(): Promise<void> {
    const epoch = this.epoch, cwd = this.root, comparison = this.comparison;
    this.refreshing = true; this.changed();
    try {
      const root = await repositoryRoot(cwd, this.abort.signal);
      const [status, branch, head] = await Promise.all([
        panelGit(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"], this.abort.signal),
        panelGit(root, ["rev-parse", "--abbrev-ref", "HEAD"], this.abort.signal).catch(() => "unborn"),
        panelGit(root, ["rev-parse", "--verify", "HEAD"], this.abort.signal).catch(() => ""),
      ]);
      const files = parseStatus(status);
      const statText = await panelGit(root, ["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z", ...comparisonArgs(comparison)], this.abort.signal).catch(() => "");
      if (this.disposed || epoch !== this.epoch) return;
      this.root = root; this.head = head.trim() || undefined; this.branch = branch.trim(); this.files = files; this.stats = parseNumstat(statText);
      this.error = undefined; this.checkedAt = Date.now(); this.revision++;
      if (this.selected === undefined) this.selected = this.visible[0]?.path;
      await this.loadSelected();
    } catch (error) {
      if (!this.disposed && epoch === this.epoch) this.error = error instanceof Error ? error.message : String(error);
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  async select(path: string): Promise<void> { this.selected = path; this.changed(); await this.loadSelected(); }
  private async loadSelected(): Promise<void> {
    const file = this.current, epoch = ++this.patchEpoch, root = this.root, comparison = this.comparison, head = this.head;
    if (!file) { this.patchSnapshot = undefined; this.patchPath = this.selected; this.patchComparison = comparison; this.patchAt = Date.now(); this.patch = this.error ?? (this.selected ? "Selected path is no longer in this comparison. Choose another file with j/k." : this.files.length ? "No files match the current filter." : "Working tree clean."); this.changed(); return; }
    try {
      const patch = await readPanelDiff(root, file, this.abort.signal, comparison, head);
      if (!this.disposed && epoch === this.patchEpoch && root === this.root && comparison === this.comparison && file.path === this.selected) {
        this.patch = patch; this.patchComparison = comparison; this.patchPath = file.path; this.patchAt = Date.now();
        this.patchSnapshot = { root, path: file.path, comparison, head, at: this.patchAt, text: patch }; this.changed();
      }
    } catch (error) {
      if (!this.disposed && epoch === this.patchEpoch) { this.patchSnapshot = undefined; this.patchPath = file.path; this.patchComparison = comparison; this.patchAt = Date.now(); this.patch = "Diff unavailable: " + (error instanceof Error ? error.message : String(error)); this.changed(); }
    }
  }
  get limitNotice(): string { return this.files.length === FILE_LIMIT ? `First ${FILE_LIMIT} paths; narrow repository scope to inspect more.` : ""; }
  dispose(): void { this.disposed = true; this.abort.abort(); }
}
