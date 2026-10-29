// The first-party LSP extension (T-252 lsp-extension): lsp_diagnostics, lsp_definition and
// lsp_references for files in the worktree. Each file extension maps to one configured server,
// started lazily by argv and kept for the session. Results are bounded in count and bytes; a
// missing server, an unsupported file or a path outside the worktree is a tool error.
import { existsSync, readFileSync, realpathSync, statSync } from "node:fs";
import { dirname, extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { DEFAULT_LIMITS, type Diagnostic, LspClient, LspError, type LspLimits, type LspServerSpec, type Range } from "./client.ts";

export { LspClient, LspError, type LspLimits, type LspServerSpec } from "./client.ts";

export const DEFAULT_SERVERS: readonly LspServerSpec[] = [
  {
    id: "rust-analyzer",
    command: ["rust-analyzer"],
    languages: { ".rs": "rust" },
    // A separate target directory keeps rust-analyzer's cargo check off the gate's build lock.
    initializationOptions: { cargo: { targetDir: true } },
    configSection: "rust-analyzer",
  },
  {
    id: "typescript-language-server",
    command: ["typescript-language-server", "--stdio"],
    languages: {
      ".ts": "typescript", ".tsx": "typescriptreact", ".mts": "typescript", ".cts": "typescript",
      ".js": "javascript", ".jsx": "javascriptreact", ".mjs": "javascript", ".cjs": "javascript",
    },
  },
];

/** Convert the generation's Rust-validated configuration into explicit argv server specs. */
export function configuredServers(settings: Readonly<Record<string, unknown>> | undefined): readonly LspServerSpec[] {
  const raw = settings?.languages;
  if (raw === undefined) return DEFAULT_SERVERS;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw new Error("invalid lsp.languages");
  const configured: LspServerSpec[] = Object.entries(raw).map(([id, value]) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`invalid lsp server ${id}`);
    const spec = value as Record<string, unknown>;
    if (typeof spec.command !== "string" || !spec.command || !spec.extensions || typeof spec.extensions !== "object" || Array.isArray(spec.extensions)
      || (spec.args !== undefined && (!Array.isArray(spec.args) || spec.args.some((v) => typeof v !== "string")))
      || (spec.root_markers !== undefined && (!Array.isArray(spec.root_markers) || spec.root_markers.some((v) => typeof v !== "string" || !v || v === "." || v === ".." || v.includes("/"))))
      || Object.entries(spec.extensions).some(([ext, lang]) => !ext.startsWith(".") || typeof lang !== "string" || !lang)) throw new Error(`invalid lsp server ${id}`);
    return { id, command: [spec.command, ...((spec.args ?? []) as string[])], languages: spec.extensions as Record<string, string>, rootMarkers: (spec.root_markers ?? []) as string[] };
  });
  return [...configured, ...DEFAULT_SERVERS];
}

export const LSP_TOOL_NAMES = ["lsp_diagnostics", "lsp_definition", "lsp_references"] as const;

export const MAX_ITEMS = 50;
export const MAX_OUTPUT_BYTES = 16 * 1024;
const MAX_MESSAGE_CHARS = 500;
const MAX_PREVIEW_CHARS = 200;
const SEVERITY = ["", "error", "warning", "information", "hint"];

export interface LspToolkitOptions {
  readonly root: string;
  readonly servers?: readonly LspServerSpec[];
  readonly limits?: Partial<LspLimits>;
}

export interface LspToolkit {
  readonly tools: readonly ToolDefinition[];
  /** Stops every server this toolkit started; the next tool call starts a fresh one. */
  shutdown(): Promise<void>;
}

interface Located {
  readonly path: string;
  readonly line: number;
  readonly column: number;
  readonly end_line: number;
  readonly end_column: number;
}

function located(root: string, uri: string, range: Range): Located {
  const absolute = uri.startsWith("file:") ? fileURLToPath(uri) : uri;
  const inside = relative(root, absolute);
  const path = inside && !inside.startsWith("..") && !isAbsolute(inside) ? inside : absolute;
  return {
    path, line: range.start.line + 1, column: range.start.character + 1,
    end_line: range.end.line + 1, end_column: range.end.character + 1,
  };
}

/** Drops trailing items until the JSON fits the byte bound; `total` keeps the server's count. */
function bounded<T>(items: readonly T[], build: (kept: readonly T[], truncated: boolean) => Record<string, unknown>): string {
  let kept = items.slice(0, MAX_ITEMS);
  for (;;) {
    const text = JSON.stringify(build(kept, kept.length < items.length));
    if (Buffer.byteLength(text) <= MAX_OUTPUT_BYTES || kept.length === 0) return text;
    kept = kept.slice(0, Math.max(0, Math.floor(kept.length * 0.75)));
  }
}

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}...` : text;
}

const PathParam = Type.String({ description: "A file in the worktree, relative to it or absolute" });
const Line = Type.Integer({ minimum: 1, description: "1-based line" });
const Column = Type.Integer({ minimum: 1, description: "1-based column (UTF-16 code units, as the editor counts)" });

export function lspToolkit(options: LspToolkitOptions): LspToolkit {
  const root = realpathSync(options.root);
  const servers = options.servers ?? DEFAULT_SERVERS;
  const limits: LspLimits = { ...DEFAULT_LIMITS, ...options.limits };
  const clients = new Map<string, Promise<LspClient>>();

  const target = (path: string): { file: string; spec: LspServerSpec; languageId: string } => {
    let file: string;
    try {
      file = realpathSync(resolve(root, path));
    } catch {
      throw new LspError("lsp_path_refused", `${path} does not exist`);
    }
    if (file !== root && !file.startsWith(root + sep)) {
      throw new LspError("lsp_path_refused", `${path} is outside the worktree ${root}`);
    }
    if (!statSync(file).isFile()) throw new LspError("lsp_path_refused", `${path} is not a regular file`);
    const extension = extname(file);
    for (const spec of servers) {
      const languageId = spec.languages[extension];
      if (languageId !== undefined) return { file, spec, languageId };
    }
    const known = servers.flatMap((spec) => Object.keys(spec.languages)).join(", ");
    throw new LspError("lsp_unsupported_file", `no language server is configured for ${extension || path} (configured: ${known})`);
  };

  const client = async (spec: LspServerSpec, file: string): Promise<LspClient> => {
    let serverRoot = dirname(file);
    while (serverRoot !== root && !spec.rootMarkers?.some((marker) => existsSync(join(serverRoot, marker)))) {
      serverRoot = dirname(serverRoot);
    }
    const key = JSON.stringify([spec.id, serverRoot]);
    const existing = clients.get(key);
    if (existing) {
      const current = await existing.catch(() => undefined);
      if (current?.alive) return current;
    }
    const started = LspClient.start(spec, serverRoot, limits);
    clients.set(key, started);
    started.catch(() => { if (clients.get(key) === started) clients.delete(key); });
    return started;
  };

  const locationQuery = async (method: string, params: { path: string; line: number; column: number },
    extra: Record<string, unknown>) => {
    const { file, spec, languageId } = target(params.path);
    const server = await client(spec, file);
    const capability = method === "textDocument/definition" ? "definitionProvider" : "referencesProvider";
    if (!server.supports(capability)) throw new LspError("lsp_request_failed", `${spec.id} does not support ${method}`);
    const { uri } = server.sync(file, languageId);
    const complete = await server.settle();
    const result = await server.request(method, {
      textDocument: { uri }, position: { line: params.line - 1, character: params.column - 1 }, ...extra,
    });
    const raw = result === null ? [] : Array.isArray(result) ? result : [result];
    const locations = raw.map((entry: Record<string, unknown>) => entry.targetUri !== undefined
      ? located(root, entry.targetUri as string, (entry.targetSelectionRange ?? entry.targetRange) as Range)
      : located(root, entry.uri as string, entry.range as Range));
    const withPreview = locations.map((location) => ({ ...location, preview: preview(root, location) }));
    return bounded(withPreview, (kept, truncated) => ({
      server: spec.id, complete, total: withPreview.length, truncated, locations: kept,
    }));
  };

  const tools: ToolDefinition[] = [
    {
      name: "lsp_diagnostics",
      label: "Language server diagnostics",
      description: "Errors and warnings the configured language server reports for one file in the worktree (for "
        + `Rust, rust-analyzer including cargo check). At most ${MAX_ITEMS} items; complete is false when the server `
        + "was still working at the deadline. Reads the file from disk, so save edits first.",
      parameters: Type.Object({ path: PathParam }),
      async execute(_id, params: { path: string }) {
        const { file, spec, languageId } = target(params.path);
        const server = await client(spec, file);
        const { uri, since } = server.sync(file, languageId);
        const complete = await server.settle(() => server.diagnostics(uri, since) !== undefined);
        const diagnostics = (server.diagnostics(uri, since) ?? []).map((item: Diagnostic) => ({
          ...located(root, uri, item.range),
          severity: SEVERITY[item.severity ?? 0] || "unknown",
          ...(item.code !== undefined ? { code: String(item.code) } : {}),
          ...(item.source ? { source: item.source } : {}),
          message: clip(item.message, MAX_MESSAGE_CHARS),
        }));
        const text = bounded(diagnostics, (kept, truncated) => ({
          server: spec.id, path: relative(root, file), complete, total: diagnostics.length, truncated, diagnostics: kept,
        }));
        return { content: [{ type: "text", text }], details: undefined };
      },
    },
    {
      name: "lsp_definition",
      label: "Go to definition",
      description: "Where the symbol at a position in a worktree file is defined, from the configured language "
        + `server. At most ${MAX_ITEMS} locations, each with a one-line preview.`,
      parameters: Type.Object({ path: PathParam, line: Line, column: Column }),
      async execute(_id, params: { path: string; line: number; column: number }) {
        const text = await locationQuery("textDocument/definition", params, {});
        return { content: [{ type: "text", text }], details: undefined };
      },
    },
    {
      name: "lsp_references",
      label: "Find references",
      description: "Every reference to the symbol at a position in a worktree file, from the configured language "
        + `server. At most ${MAX_ITEMS} locations; total is the server's full count.`,
      parameters: Type.Object({
        path: PathParam, line: Line, column: Column,
        include_declaration: Type.Optional(Type.Boolean({ description: "Include the declaration itself (default true)" })),
      }),
      async execute(_id, params: { path: string; line: number; column: number; include_declaration?: boolean }) {
        const text = await locationQuery("textDocument/references", params, {
          context: { includeDeclaration: params.include_declaration ?? true },
        });
        return { content: [{ type: "text", text }], details: undefined };
      },
    },
  ];

  return {
    tools,
    async shutdown() {
      const running = [...clients.values()];
      clients.clear();
      await Promise.all(running.map((started) => started.then((server) => server.shutdown(), () => undefined)));
    },
  };
}

function preview(root: string, location: Located): string {
  try {
    const file = isAbsolute(location.path) ? location.path : resolve(root, location.path);
    const line = readLine(file, location.line);
    return clip(line.trim(), MAX_PREVIEW_CHARS);
  } catch {
    return "";
  }
}

const PREVIEW_FILE_MAX_BYTES = 4 * 1024 * 1024;

function readLine(file: string, line: number): string {
  if (statSync(file).size > PREVIEW_FILE_MAX_BYTES) return "";
  return readFileSync(file, "utf8").split("\n")[line - 1] ?? "";
}
