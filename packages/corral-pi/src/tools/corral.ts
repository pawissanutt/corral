import { createHash, randomUUID } from "node:crypto";
import { closeSync, mkdirSync, openSync, readFileSync, renameSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { TOML } from "bun";
import { engineRun, type Run } from "../build/corral-cli.ts";
import type { HostIdentity } from "../roles/types.ts";
import { transcriptPage, type HistoryRequest } from "./history.ts";

/** Stable JSON for host-local comparisons and hashes. */
export function canonicalJson(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" && Number.isSafeInteger(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>)
      .filter(([, item]) => item !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`).join(",")}}`;
  }
  throw new TypeError(`cannot encode ${typeof value} as canonical JSON`);
}

export function sha256Hex(data: string | Uint8Array): string {
  return createHash("sha256").update(data).digest("hex");
}

export interface CorralClientOptions {
  readonly corral: string;
  readonly root: string;
  readonly cwd: string;
  readonly sessionDir?: string;
  readonly identity: HostIdentity;
}

export class CorralCommandError extends Error {
  constructor(readonly reason: string, readonly next?: readonly unknown[], readonly diagnostic?: string) {
    super(diagnostic ?? reason);
  }
}

export interface ModelUsage {
  readonly model: string;
  readonly input: number;
  readonly output: number;
  readonly cache_read: number;
  readonly cache_write: number;
  readonly usd?: number;
  readonly billing?: "subscription" | "api";
}

/** The same engine argv an owner would run. No hidden transport or effect retries. */
export class CorralClient {
  private readonly run: Run;
  constructor(private readonly options: CorralClientOptions) {
    this.run = engineRun(options.corral);
  }

  get workspaceRoot(): string { return this.options.root; }
  get configured(): boolean { return this.options.corral !== ""; }

  private async execute(argv: readonly string[], json: boolean): Promise<string> {
    if (!this.configured) throw new CorralCommandError("No engine configured; pass --corral <path>");
    const result = await this.run(["--root", this.options.root, ...(json ? ["--json"] : []), ...argv],
      { cwd: this.options.cwd });
    if (result.code === 0) return result.stdout;
    const diagnostic = result.stderr || result.stdout || `${argv[0]} exited ${result.code}`;
    for (const text of [result.stdout, result.stderr]) {
      try {
        const refusal = JSON.parse(text) as { reason?: unknown; next?: unknown } | null;
        if (typeof refusal?.reason === "string" && Array.isArray(refusal.next)) {
          throw new CorralCommandError(refusal.reason, refusal.next);
        }
      } catch (error) { if (error instanceof CorralCommandError) throw error; }
    }
    // The current CLI prints its refusal reason and exact corrective commands to stderr even
    // in JSON mode. Preserve both the raw text and those complete commands for the model.
    const [first, ...lines] = diagnostic.split("\n");
    if (first?.startsWith("refused: ")) {
      const firstNext = lines.findIndex(line => line.startsWith("next: "));
      const reason = [first.slice(9), ...lines.slice(0, firstNext < 0 ? undefined : firstNext)].join("\n");
      const next = firstNext < 0 ? [] : lines.slice(firstNext).filter(line => line.startsWith("next: ")).map(line => line.slice(6));
      throw new CorralCommandError(reason, next, diagnostic);
    }
    throw new CorralCommandError(diagnostic);
  }

  async command(argv: readonly string[]): Promise<unknown> {
    const stdout = await this.execute(argv, true);
    try { return JSON.parse(stdout); }
    catch (cause) { throw new Error(`${argv[0]} returned invalid JSON`, { cause }); }
  }

  artifact(task: string, path: string, kind?: string, title?: string): Promise<unknown> {
    return this.command(["artifact", "add", task, path.startsWith("-") ? resolve(this.options.cwd, path) : path,
      ...(kind ? kind.startsWith("-") ? [`--kind=${kind}`] : ["--kind", kind] : []),
      ...(title ? title.startsWith("-") ? [`--title=${title}`] : ["--title", title] : [])]);
  }

  usage(usage: ModelUsage): Promise<unknown> {
    const { task, run } = this.options.identity;
    return this.command(["usage", "add", "--model", usage.model,
      ...(task ? ["--task", task] : []), ...(run ? ["--run", run] : []),
      "--input", String(usage.input), "--output", String(usage.output),
      "--cache-read", String(usage.cache_read), "--cache-write", String(usage.cache_write),
      ...(usage.usd === undefined ? [] : ["--usd", String(usage.usd)]),
      ...(usage.billing ? ["--billing", usage.billing] : [])]);
  }

  taskSnapshot(): Promise<unknown> {
    const { task } = this.options.identity;
    if (!task) throw new CorralCommandError("No task is attached to this session");
    return this.command(["show", task]);
  }

  artifacts(): Promise<unknown> {
    const { task } = this.options.identity;
    return this.command(["artifact", "list", ...(task ? [task] : [])]);
  }

  async artifactFile(reference: string): Promise<string> {
    if (!artifactReference(reference)) throw new CorralCommandError("Invalid artifact reference; use corral artifact list");
    if (!this.configured || !this.options.sessionDir) throw new CorralCommandError("Artifact preview needs a local session and --corral <path>");
    const dir = join(this.options.sessionDir, "artifact-previews");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const path = join(dir, reference);
    const temp = join(dir, randomUUID());
    const fd = openSync(temp, "wx", 0o600);
    try {
      const { code, stderr } = await this.run(["--root", this.options.root, "artifact", "show", reference],
        { cwd: this.options.cwd, stdoutFd: fd });
      if (code !== 0) throw new CorralCommandError(stderr || `artifact show exited ${code}`);
      renameSync(temp, path);
      return path;
    } finally {
      closeSync(fd);
      rmSync(temp, { force: true });
    }
  }

  async history(request: HistoryRequest): Promise<unknown> {
    if (request.kind === "transcript") return transcriptPage(this.options.root, request);
    if (request.kind === "task") return this.command(["show", request.task]);
    const tasks = await this.command(["list", "--all"]);
    const query = request.query.trim().toLowerCase();
    if (!query || !Array.isArray(tasks)) return tasks;
    return tasks.filter(task => [task?.id, task?.title, task?.body].some(value =>
      typeof value === "string" && value.toLowerCase().includes(query)));
  }

  async projectScopes(): Promise<unknown> {
    const { root } = this.options;
    let projects: unknown = {};
    try {
      const config = TOML.parse(readFileSync(join(root, ".corral-next", "corral.toml"), "utf8")) as { projects?: unknown };
      projects = config.projects ?? {};
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
    return { workspace: root, projects };
  }
}

export function artifactReference(reference: string): boolean {
  return /^T-\d+:[1-9]\d*$/.test(reference);
}
