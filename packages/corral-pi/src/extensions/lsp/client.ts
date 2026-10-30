// One language server process, spawned by argv (never through a shell) and spoken to over
// stdio with JSON-RPC. Every wait is bounded; a server that is missing, exits or breaks the
// protocol turns into an LspError for the calling tool, never an unhandled host exception.
import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import { accessSync, constants, readFileSync, statSync } from "node:fs";
import { delimiter, isAbsolute, join } from "node:path";
import { pathToFileURL } from "node:url";

import { FrameDecoder, encodeFrame } from "./framing.ts";

export interface LspServerSpec {
  /** Stable name used in tool output and errors, for example `rust-analyzer`. */
  readonly id: string;
  /** argv; the first element is resolved on PATH unless it is a path. */
  readonly command: readonly string[];
  /** The LSP language ID of each file extension (with the dot) this server answers for. */
  readonly languages: Readonly<Record<string, string>>;
  /** Find the nearest ancestor containing one of these names, bounded by the worktree. */
  readonly rootMarkers?: readonly string[];
  readonly initializationOptions?: Record<string, unknown>;
  /** The `workspace/configuration` section answered with `initializationOptions`. */
  readonly configSection?: string;
}

export interface LspLimits {
  readonly requestTimeoutMs: number;
  /** How long a query waits for the server's own progress (indexing, cargo check) to finish. */
  readonly readyTimeoutMs: number;
  /** How long a server must report nothing new before it counts as settled. */
  readonly quietMs: number;
  readonly maxFrameBytes: number;
}

export const DEFAULT_LIMITS: LspLimits = {
  requestTimeoutMs: 30_000,
  readyTimeoutMs: 120_000,
  quietMs: 500,
  maxFrameBytes: 16 * 1024 * 1024,
};

export type LspErrorCode =
  | "lsp_server_unavailable" | "lsp_server_exited" | "lsp_protocol_error" | "lsp_request_failed"
  | "lsp_timeout" | "lsp_unsupported_file" | "lsp_path_refused";

export class LspError extends Error {
  constructor(readonly code: LspErrorCode, reason: string) {
    super(`${code}: ${reason}`);
  }
}

export interface Position {
  readonly line: number;
  readonly character: number;
}

export interface Range {
  readonly start: Position;
  readonly end: Position;
}

export interface Diagnostic {
  readonly range: Range;
  readonly severity?: number;
  readonly code?: string | number;
  readonly source?: string;
  readonly message: string;
}

const STDERR_TAIL_BYTES = 4 * 1024;

/** Resolves argv[0] to an executable file, or explains why the server is unavailable. */
export function resolveCommand(spec: LspServerSpec, path = process.env.PATH ?? ""): string {
  const program = spec.command[0];
  if (!program) throw new LspError("lsp_server_unavailable", `${spec.id} has an empty command`);
  const executable = (candidate: string) => {
    try {
      accessSync(candidate, constants.X_OK);
      return statSync(candidate).isFile();
    } catch {
      return false;
    }
  };
  if (program.includes("/")) {
    if (isAbsolute(program) && executable(program)) return program;
    throw new LspError("lsp_server_unavailable", `${spec.id}: ${program} is not an executable file`);
  }
  for (const dir of path.split(delimiter)) {
    if (dir && executable(join(dir, program))) return join(dir, program);
  }
  throw new LspError("lsp_server_unavailable", `${spec.id}: ${program} is not on PATH; install it to use this tool`);
}

interface Pending {
  readonly method: string;
  resolve(value: unknown): void;
  reject(error: Error): void;
  readonly timer: ReturnType<typeof setTimeout>;
}

interface Document {
  version: number;
  text: string;
}

export class LspClient {
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly documents = new Map<string, Document>();
  private readonly published = new Map<string, { seq: number; diagnostics: Diagnostic[] }>();
  private readonly progress = new Set<string | number>();
  private readonly listeners = new Set<() => void>();
  private activity = 0;
  private stderr = "";
  private failure: LspError | undefined;
  private capabilities: Record<string, unknown> = {};

  private constructor(
    readonly spec: LspServerSpec,
    readonly root: string,
    private readonly limits: LspLimits,
    private readonly child: ChildProcessWithoutNullStreams,
  ) {}

  static async start(spec: LspServerSpec, root: string, limits: LspLimits = DEFAULT_LIMITS): Promise<LspClient> {
    const program = resolveCommand(spec);
    const child = spawn(program, spec.command.slice(1), { cwd: root, stdio: ["pipe", "pipe", "pipe"], shell: false });
    const client = new LspClient(spec, root, limits, child);
    client.attach();
    const init = await client.request("initialize", {
      processId: process.pid,
      rootUri: pathToFileURL(root).href,
      workspaceFolders: [{ uri: pathToFileURL(root).href, name: "worktree" }],
      capabilities: {
        window: { workDoneProgress: true },
        textDocument: {
          synchronization: { didSave: true },
          publishDiagnostics: { versionSupport: true },
          definition: { linkSupport: true },
          references: {},
        },
        workspace: { workspaceFolders: true, configuration: true },
      },
      ...(spec.initializationOptions ? { initializationOptions: spec.initializationOptions } : {}),
    }) as { capabilities?: Record<string, unknown> } | null;
    client.capabilities = init?.capabilities ?? {};
    client.notify("initialized", {});
    return client;
  }

  get alive(): boolean {
    return this.failure === undefined;
  }

  supports(capability: string): boolean {
    const value = this.capabilities[capability];
    return value !== undefined && value !== null && value !== false;
  }

  private attach(): void {
    const decoder = new FrameDecoder(this.limits.maxFrameBytes);
    this.child.stdout.on("data", (chunk: Buffer) => {
      if (this.failure) return;
      let messages: unknown[];
      try {
        messages = decoder.push(chunk);
      } catch (error) {
        this.fail(new LspError("lsp_protocol_error", `${this.spec.id}: ${(error as Error).message}`));
        this.child.kill("SIGKILL");
        return;
      }
      for (const message of messages) this.dispatch(message as Record<string, unknown>);
    });
    this.child.stderr.on("data", (chunk: Buffer) => {
      this.stderr = (this.stderr + chunk.toString("utf8")).slice(-STDERR_TAIL_BYTES);
    });
    this.child.stdin.on("error", () => undefined);
    this.child.on("error", (error) => {
      this.fail(new LspError("lsp_server_unavailable", `${this.spec.id}: ${error.message}`));
    });
    this.child.on("exit", (code, signal) => {
      const tail = this.stderr.trim().split("\n").slice(-5).join("\n");
      this.fail(new LspError("lsp_server_exited",
        `${this.spec.id} exited (${signal ?? `code ${code}`})${tail ? `; stderr: ${tail}` : ""}`));
    });
  }

  private fail(error: LspError): void {
    if (this.failure) return;
    this.failure = error;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    this.touch();
  }

  private touch(): void {
    this.activity++;
    for (const listener of [...this.listeners]) listener();
  }

  private send(message: Record<string, unknown>): void {
    if (this.failure) throw this.failure;
    this.child.stdin.write(encodeFrame({ jsonrpc: "2.0", ...message }));
  }

  private dispatch(message: Record<string, unknown>): void {
    const id = message.id;
    const method = typeof message.method === "string" ? message.method : undefined;
    if (method !== undefined && id !== undefined) {
      this.answerServerRequest(id, method, message.params);
      return;
    }
    if (method !== undefined) {
      this.notification(method, message.params as Record<string, unknown> | undefined);
      return;
    }
    if (typeof id !== "number") return;
    const pending = this.pending.get(id);
    if (!pending) return;
    this.pending.delete(id);
    clearTimeout(pending.timer);
    const error = message.error as { code?: number; message?: string } | undefined;
    if (error) {
      pending.reject(new LspError("lsp_request_failed", `${this.spec.id} ${pending.method}: ${error.message ?? "error"}`
        + (error.code !== undefined ? ` (${error.code})` : "")));
    } else {
      pending.resolve(message.result ?? null);
    }
  }

  private answerServerRequest(id: unknown, method: string, params: unknown): void {
    if (method === "workspace/configuration") {
      const items = (params as { items?: unknown[] } | undefined)?.items ?? [];
      const section = (item: unknown) => (item as { section?: string } | null)?.section;
      this.send({ id, result: items.map((item) => this.spec.configSection !== undefined
        && section(item) === this.spec.configSection ? this.spec.initializationOptions ?? null : null) });
    } else if (method === "window/workDoneProgress/create" || method === "client/registerCapability"
      || method === "client/unregisterCapability") {
      this.send({ id, result: null });
    } else {
      this.send({ id, error: { code: -32601, message: `${method} is not supported by this client` } });
    }
  }

  private notification(method: string, params: Record<string, unknown> | undefined): void {
    if (method === "$/progress" && params) {
      const token = params.token as string | number;
      const kind = (params.value as { kind?: string } | undefined)?.kind;
      if (kind === "begin") this.progress.add(token);
      else if (kind === "end") this.progress.delete(token);
      this.touch();
    } else if (method === "textDocument/publishDiagnostics" && params && typeof params.uri === "string") {
      this.published.set(params.uri, {
        seq: this.activity + 1, diagnostics: (params.diagnostics as Diagnostic[] | undefined) ?? [],
      });
      this.touch();
    }
  }

  request(method: string, params: unknown, timeoutMs = this.limits.requestTimeoutMs): Promise<unknown> {
    if (this.failure) return Promise.reject(this.failure);
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new LspError("lsp_timeout", `${this.spec.id} did not answer ${method} within ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(id, { method, resolve, reject, timer });
      try {
        this.send({ id, method, params });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(error);
      }
    });
  }

  notify(method: string, params: unknown): void {
    this.send({ method, params });
  }

  /**
   * Opens the file, or sends its current disk content when it changed; returns its URI and the
   * sync point. An unchanged open file sends nothing, so diagnostics already published count.
   */
  sync(path: string, languageId: string): { uri: string; since: number } {
    const uri = pathToFileURL(path).href;
    const text = readFileSync(path, "utf8");
    const open = this.documents.get(uri);
    const since = open?.text === text ? -1 : this.activity;
    if (!open) {
      this.documents.set(uri, { version: 1, text });
      this.notify("textDocument/didOpen", { textDocument: { uri, languageId, version: 1, text } });
      this.notify("textDocument/didSave", { textDocument: { uri } });
    } else if (open.text !== text) {
      open.version++;
      open.text = text;
      this.notify("textDocument/didChange", { textDocument: { uri, version: open.version }, contentChanges: [{ text }] });
      this.notify("textDocument/didSave", { textDocument: { uri } });
    }
    return { uri, since };
  }

  /**
   * Waits until the server has no open progress and reported nothing for `quietMs`, and `ready`
   * (when given) holds. Resolves false at the ready deadline instead of failing: a slow index
   * still answers, and the tool says the answer may be incomplete.
   */
  settle(ready: () => boolean = () => true, timeoutMs = this.limits.readyTimeoutMs): Promise<boolean> {
    return new Promise((resolve, reject) => {
      let quiet: ReturnType<typeof setTimeout> | undefined;
      const finish = (settled: boolean | Error) => {
        clearTimeout(deadline);
        clearTimeout(quiet);
        this.listeners.delete(check);
        if (settled instanceof Error) reject(settled);
        else resolve(settled);
      };
      const check = () => {
        clearTimeout(quiet);
        if (this.failure) {
          finish(this.failure);
          return;
        }
        if (this.progress.size > 0 || !ready()) return;
        quiet = setTimeout(() => finish(true), this.limits.quietMs);
      };
      const deadline = setTimeout(() => finish(false), timeoutMs);
      this.listeners.add(check);
      check();
    });
  }

  /** The newest diagnostics the server published for `uri` after the sync point `since`, if any. */
  diagnostics(uri: string, since: number): Diagnostic[] | undefined {
    const entry = this.published.get(uri);
    return entry && entry.seq > since ? entry.diagnostics : undefined;
  }

  async shutdown(timeoutMs = 2_000): Promise<void> {
    if (this.failure) return;
    try {
      await this.request("shutdown", null, timeoutMs);
      this.notify("exit", null);
    } catch {
      // A server that cannot shut down cleanly is killed below; there is no caller to tell.
    }
    if (this.child.exitCode === null) this.child.kill("SIGKILL");
    this.fail(new LspError("lsp_server_exited", `${this.spec.id} was shut down`));
  }
}
