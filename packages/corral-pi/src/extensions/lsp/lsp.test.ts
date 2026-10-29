// The LSP extension against the scripted fake server (started by argv) and against a missing
// server: bounded results, 1-based positions, refusals and server failures as LspError.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";

import { FrameDecoder, encodeFrame } from "./framing.ts";
import { DEFAULT_SERVERS, LSP_TOOL_NAMES, LspError, type LspServerSpec, MAX_ITEMS, MAX_OUTPUT_BYTES, configuredServers, lspToolkit } from "./index.ts";

const FAKE = fileURLToPath(new URL("./testing/fake-server.ts", import.meta.url));
const LIMITS = { quietMs: 0, readyTimeoutMs: 10_000, requestTimeoutMs: 10_000 };
const fake = (...flags: string[]): LspServerSpec => ({
  id: "fake", command: [process.execPath, FAKE, ...flags], languages: { ".fk": "fake" },
  initializationOptions: { answer: 42 }, configSection: "fake",
});

let root: string;
const toolkits: { shutdown(): Promise<void> }[] = [];
const kit = (spec: LspServerSpec) => {
  const made = lspToolkit({ root, servers: [spec], limits: LIMITS });
  toolkits.push(made);
  return made;
};

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), "corral-pi-lsp-"));
  mkdirSync(join(root, "src"));
  writeFileSync(join(root, "src/main.fk"), "def alpha\nuse alpha ERR one\nok\nuse alpha ERR two\n");
  writeFileSync(join(root, "src/many.fk"), `def beta\n${"use beta\n".repeat(120)}`);
  writeFileSync(join(root, "notes.txt"), "text\n");
});

afterAll(async () => {
  await Promise.all(toolkits.map((made) => made.shutdown()));
  rmSync(root, { recursive: true, force: true });
});

function tool(made: { tools: readonly ToolDefinition[] }, name: string): ToolDefinition {
  const found = made.tools.find((candidate) => candidate.name === name);
  if (!found) throw new Error(`no ${name}`);
  return found;
}

async function call(made: { tools: readonly ToolDefinition[] }, name: string, params: Record<string, unknown>) {
  const result = await tool(made, name).execute("call-1", params as never, undefined, undefined, undefined as never);
  const part = result.content[0];
  return JSON.parse(part && part.type === "text" ? part.text : "null");
}

async function failure(promise: Promise<unknown>): Promise<LspError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof LspError) return error;
    throw error;
  }
  throw new Error("expected an LspError");
}

test("framing round-trips split chunks and refuses a missing header or an oversize frame", () => {
  const frame = encodeFrame({ id: 1, text: "héllo" });
  const decoder = new FrameDecoder(1024);
  expect(decoder.push(frame.subarray(0, 7))).toEqual([]);
  expect(decoder.push(Buffer.concat([frame.subarray(7), frame]))).toEqual([{ id: 1, text: "héllo" }, { id: 1, text: "héllo" }]);
  expect(() => new FrameDecoder(1024).push(Buffer.from("X-Other: 1\r\n\r\n{}"))).toThrow(/no Content-Length/);
  expect(() => new FrameDecoder(8).push(encodeFrame({ long: "0123456789" }))).toThrow(/exceeds the 8-byte limit/);
});

test("the extension exposes the three tools and configures rust-analyzer by argv for Rust files", () => {
  expect(lspToolkit({ root: tmpdir() }).tools.map((t) => t.name)).toEqual([...LSP_TOOL_NAMES]);
  const rust = DEFAULT_SERVERS.find((spec) => spec.languages[".rs"] === "rust");
  expect(rust?.command).toEqual(["rust-analyzer"]);
});

test("generation settings select a configured server without exposing mutating LSP tools", async () => {
  const settings = { languages: { example: { command: process.execPath, args: [FAKE], extensions: { ".fk": "fake" }, root_markers: ["marker"] } } };
  const made = lspToolkit({ root, servers: configuredServers(settings), limits: LIMITS });
  toolkits.push(made);
  expect(made.tools.map((tool) => tool.name)).toEqual([...LSP_TOOL_NAMES]);
  expect((await call(made, "lsp_definition", { path: "src/main.fk", line: 1, column: 1 })).server).toBe("example");
  expect(configuredServers({ languages: { changed: { ...settings.languages.example, args: [FAKE, "--exit-on-definition"] } } })[0]?.id).toBe("changed");
});

test("root markers select the nearest project and otherwise fall back to the worktree", async () => {
  mkdirSync(join(root, "project", "src"), { recursive: true });
  writeFileSync(join(root, "project", "marker"), "");
  writeFileSync(join(root, "project", "src", "test.fk"), "ERR one");
  const made = kit({ ...fake("--report-cwd"), rootMarkers: ["marker"] });
  const nested = await call(made, "lsp_diagnostics", { path: "project/src/test.fk" });
  const fallback = await call(made, "lsp_diagnostics", { path: "src/main.fk" });
  expect(nested.diagnostics.find((item: { code: string }) => item.code === "cwd").message).toBe(join(root, "project"));
  expect(fallback.diagnostics.find((item: { code: string }) => item.code === "cwd").message).toBe(root);
});

test("a missing server is an explicit lsp_server_unavailable tool error and the toolkit keeps working", async () => {
  const missing = kit({ id: "ghost", command: ["corral-no-such-language-server"], languages: { ".fk": "fake" } });
  const error = await failure(call(missing, "lsp_diagnostics", { path: "src/main.fk" }));
  expect(error.code).toBe("lsp_server_unavailable");
  expect(error.message).toBe("lsp_server_unavailable: ghost: corral-no-such-language-server is not on PATH; install it to use this tool");
  expect((await failure(call(missing, "lsp_definition", { path: "src/main.fk", line: 1, column: 5 }))).code)
    .toBe("lsp_server_unavailable");
  const absolute = kit({ id: "gone", command: ["/nonexistent/lsp"], languages: { ".fk": "fake" } });
  expect((await failure(call(absolute, "lsp_references", { path: "src/main.fk", line: 1, column: 5 }))).message)
    .toBe("lsp_server_unavailable: gone: /nonexistent/lsp is not an executable file");
});

test("diagnostics are 1-based, follow edits on disk, and the server receives its configuration section", async () => {
  const made = kit(fake());
  const first = await call(made, "lsp_diagnostics", { path: "src/main.fk" });
  expect(first.complete).toBeTrue();
  expect(first.server).toBe("fake");
  expect(first.path).toBe(join("src", "main.fk"));
  const errors = first.diagnostics.filter((d: { severity: string }) => d.severity === "error");
  expect(errors).toEqual([
    { path: join("src", "main.fk"), line: 2, column: 11, end_line: 2, end_column: 14, severity: "error", code: "E0001", source: "fake", message: "use alpha ERR one" },
    { path: join("src", "main.fk"), line: 4, column: 11, end_line: 4, end_column: 14, severity: "error", code: "E0001", source: "fake", message: "use alpha ERR two" },
  ]);
  const config = first.diagnostics.find((d: { code: string }) => d.code === "config");
  expect(JSON.parse(config.message)).toEqual([{ answer: 42 }, null]);
  const unchanged = await call(made, "lsp_diagnostics", { path: "src/main.fk" });
  expect(unchanged.complete).toBeTrue();
  expect(unchanged.diagnostics).toEqual(first.diagnostics);
  writeFileSync(join(root, "src/main.fk"), "def alpha\nuse alpha\nok\nuse alpha ERR two\n");
  const second = await call(made, "lsp_diagnostics", { path: join(root, "src/main.fk") });
  expect(second.diagnostics.filter((d: { severity: string }) => d.severity === "error").map((d: { line: number }) => d.line)).toEqual([4]);
});

test("definition and references return worktree-relative locations with previews", async () => {
  const made = kit(fake());
  const definition = await call(made, "lsp_definition", { path: "src/main.fk", line: 2, column: 5 });
  expect(definition.locations).toEqual([
    { path: join("src", "main.fk"), line: 1, column: 1, end_line: 1, end_column: 5, preview: "def alpha" },
  ]);
  const references = await call(made, "lsp_references", { path: "src/main.fk", line: 1, column: 5, include_declaration: false });
  expect(references.locations.map((l: { line: number }) => l.line)).toEqual([2, 4]);
  expect(references.truncated).toBeFalse();
  const withDeclaration = await call(made, "lsp_references", { path: "src/main.fk", line: 1, column: 5 });
  expect(withDeclaration.total).toBe(3);
});

test("results are bounded in count and bytes; total keeps the server's count", async () => {
  const made = kit(fake());
  const references = await call(made, "lsp_references", { path: "src/many.fk", line: 1, column: 5, include_declaration: false });
  expect(references.total).toBe(120);
  expect(references.truncated).toBeTrue();
  expect(references.locations).toHaveLength(MAX_ITEMS);
  expect(Buffer.byteLength(JSON.stringify(references))).toBeLessThanOrEqual(MAX_OUTPUT_BYTES);
});

test("paths outside the worktree, missing files and unconfigured file types are refused", async () => {
  const made = kit(fake());
  expect((await failure(call(made, "lsp_diagnostics", { path: "../outside.fk" }))).code).toBe("lsp_path_refused");
  expect((await failure(call(made, "lsp_diagnostics", { path: "/etc/hostname" }))).message).toMatch(/lsp_path_refused: .*(outside the worktree|does not exist)/);
  expect((await failure(call(made, "lsp_diagnostics", { path: "src" }))).message).toBe("lsp_path_refused: src is not a regular file");
  expect((await failure(call(made, "lsp_diagnostics", { path: "notes.txt" }))).message)
    .toBe("lsp_unsupported_file: no language server is configured for .txt (configured: .fk)");
});

test("a server that exits mid-request is lsp_server_exited, and the next call starts a fresh server", async () => {
  const made = kit(fake("--exit-on-definition"));
  const error = await failure(call(made, "lsp_definition", { path: "src/main.fk", line: 2, column: 5 }));
  expect(error.code).toBe("lsp_server_exited");
  expect(error.message).toMatch(/fake exited \(code 3\)/);
  const again = await call(made, "lsp_diagnostics", { path: "src/main.fk" });
  expect(again.complete).toBeTrue();
});

test("an answer over the frame limit is lsp_protocol_error, not unbounded memory", async () => {
  const made = lspToolkit({ root, servers: [fake("--oversize-definition")], limits: { ...LIMITS, maxFrameBytes: 32 * 1024 } });
  toolkits.push(made);
  const error = await failure(call(made, "lsp_definition", { path: "src/main.fk", line: 2, column: 5 }));
  expect(error.code).toBe("lsp_protocol_error");
});
