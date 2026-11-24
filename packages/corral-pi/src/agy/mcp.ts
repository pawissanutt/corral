// Private Streamable HTTP endpoint for one dormant agy ACP session. The caller owns
// tool authorization and execution; possession of this endpoint is not a sandbox.
import { randomBytes } from "node:crypto";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { Socket } from "node:net";
import type { ImageContent, TextContent, Tool } from "@earendil-works/pi-ai";
import { flatObjectSchema, type JsonSchema } from "../tools/schema.ts";
import { AcpFailure, record } from "./acp.ts";

export interface McpResult {
  content: (TextContent | ImageContent)[];
  isError?: boolean;
}
export interface AgyMcp {
  descriptor: { type: "http"; name: string; url: string; headers: { name: string; value: string }[] };
  close(): Promise<void>;
}

type Id = number | string;
type Json = Record<string, unknown>;
const ADDRESS = "127.0.0.1";
const VERSION = "2025-11-25";
const BODY_BYTES = 1024 * 1024;
const RESULT_BYTES = 8 * BODY_BYTES;
const MAX_CONNECTIONS = 16;
const MAX_CALLS = 8;
const MAX_REQUESTS = 8192;
const HTTP_MS = 5_000;
const CALL_MS = 300_000;

function send(res: ServerResponse, status: number, body?: string): void {
  if (res.destroyed || res.writableEnded) return;
  res.writeHead(status, body === undefined
    ? { Connection: "close", "Cache-Control": "no-store" }
    : { Connection: "close", "Cache-Control": "no-store", "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) });
  res.end(body);
}

function error(res: ServerResponse, id: Id | null, code: number, message: string, status = 200): void {
  send(res, status, JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } }));
}

function success(res: ServerResponse, id: Id, result: string): void {
  const body = `{"jsonrpc":"2.0","id":${JSON.stringify(id)},"result":${result}}`;
  if (Buffer.byteLength(body) > RESULT_BYTES) return error(res, id, -32603, "Result too large");
  send(res, 200, body);
}

async function readJson(req: IncomingMessage): Promise<{ value: unknown } | { status: 413 }> {
  if (Number(req.headers["content-length"]) > BODY_BYTES) return { status: 413 };
  let bytes = 0;
  const chunks: Buffer[] = [];
  for await (const chunk of req.iterator({ destroyOnReturn: false }) as AsyncIterable<Buffer>) {
    bytes += chunk.length;
    if (bytes > BODY_BYTES) return { status: 413 };
    chunks.push(chunk);
  }
  // Do not accept a truncated frame or repair invalid UTF-8 before parsing JSON.
  if (!req.complete) throw new Error();
  return { value: JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks, bytes))) };
}

function encodeToolResult(result: McpResult): string {
  // Copy only MCP content fields (for example, Pi text signatures are not MCP fields).
  if (!Array.isArray(result.content) || result.content.length > 4096 ||
      (result.isError !== undefined && typeof result.isError !== "boolean")) throw new Error();
  let bytes = 0;
  const text = (value: unknown) => {
    if (typeof value !== "string" || (bytes += Buffer.byteLength(value)) > RESULT_BYTES) throw new Error();
    return value;
  };
  const content = result.content.map((block: TextContent | ImageContent) => {
    if (block.type === "text") return { type: "text", text: text(block.text) };
    if (block.type === "image") return { type: "image", data: text(block.data), mimeType: text(block.mimeType) };
    throw new Error();
  });
  return JSON.stringify({ content, isError: result.isError });
}

export async function openAgyMcp(
  tools: readonly Tool[],
  onCall: (id: Id, name: string, args: Json, meta: Json | undefined, signal: AbortSignal) => Promise<McpResult>,
  signal: AbortSignal,
): Promise<AgyMcp> {
  const aborted = () => new AcpFailure("aborted", "MCP bridge cancelled");
  if (signal.aborted) throw aborted();
  const names = new Set(tools.map(tool => tool.name));
  let list: string;
  try {
    list = JSON.stringify({ tools: tools.map(({ name, description, parameters }) => ({
      name, description, inputSchema: flatObjectSchema(parameters as JsonSchema),
    })) });
    if (Buffer.byteLength(list) > RESULT_BYTES) throw new Error();
  } catch {
    throw new AcpFailure("launch", "invalid MCP tool declarations");
  }

  // A unique server name prevents a configured ambient server from sharing our ACP prefix.
  const serverName = `corral-${randomBytes(8).toString("hex")}`;
  const path = `/mcp/${randomBytes(32).toString("hex")}`;
  // A socket starts with a header deadline; undefined marks its one claimed request.
  const sockets = new Map<Socket, ReturnType<typeof setTimeout> | undefined>();
  const activeCalls = new Set<AbortController>();
  let host: string | undefined;
  let closed = false;
  let closing: Promise<void> | undefined;
  let requests = 0;
  let state: "new" | "initializing" | "ready" = "new";

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    if (closed || signal.aborted) { res.destroy(); return; }
    if (!host || req.headers.host !== host || req.url !== path) return send(res, 404);
    if (req.headers.origin !== undefined) return send(res, 403);
    if (req.method !== "POST") {
      res.setHeader("Allow", "POST");
      return send(res, 405);
    }
    if (!/^application\/json\s*(?:;|$)/i.test(req.headers["content-type"] ?? "")) return send(res, 415);
    const protocol = req.headers["mcp-protocol-version"];
    if (protocol !== undefined && protocol !== VERSION) return send(res, 400);
    if (++requests > MAX_REQUESTS) return send(res, 429);

    const controller = new AbortController();
    let timer = setTimeout(() => res.destroy(), HTTP_MS);
    res.once("close", () => {
      clearTimeout(timer);
      if (!res.writableFinished) controller.abort();
    });
    const isLive = () => !closed && !signal.aborted && !controller.signal.aborted && !res.destroyed && !req.socket.destroyed;
    let body: Awaited<ReturnType<typeof readJson>>;
    try { body = await readJson(req); }
    catch { if (isLive()) error(res, null, -32700, "Parse error", 400); return; }
    if (!isLive()) return;
    if ("status" in body) return send(res, body.status);
    const message = record(body.value);
    const rawId = message?.id;
    const id = typeof rawId === "string" || typeof rawId === "number" && Number.isSafeInteger(rawId) ? rawId : null;
    const notification = message !== undefined && !("id" in message);
    if (message?.jsonrpc !== "2.0" || typeof message.method !== "string" ||
        (!notification && id === null) || "result" in message || "error" in message) {
      return error(res, null, -32600, "Invalid request", 400);
    }
    const params = record(message.params);
    const meta = record(params?._meta);
    const invalidParams = () => id === null ? send(res, 400) : error(res, id, -32602, "Invalid params");
    if (("params" in message && !params) || (params && "_meta" in params && !meta)) return invalidParams();

    if (id === null) {
      if (message.method !== "notifications/initialized" || state === "new") return send(res, 400);
      state = "ready";
      return send(res, 202);
    }
    if (message.method === "initialize") {
      const info = record(params?.clientInfo);
      if (typeof params?.protocolVersion !== "string" || !params.protocolVersion || !record(params.capabilities) ||
          typeof info?.name !== "string" || typeof info.version !== "string") return invalidParams();
      if (state === "new") state = "initializing";
      return success(res, id, JSON.stringify({
        protocolVersion: VERSION, capabilities: { tools: {} }, serverInfo: { name: serverName, version: "1" },
      }));
    }
    if (message.method === "ping") return success(res, id, "{}");
    if (message.method !== "tools/list" && message.method !== "tools/call") return error(res, id, -32601, "Method not found");
    if (state !== "ready") return error(res, id, -32000, "Server not initialized");
    if (message.method === "tools/list") return params && "cursor" in params ? invalidParams() : success(res, id, list);

    const name = params?.name;
    const args = params && "arguments" in params ? record(params.arguments) : {};
    if (typeof name !== "string" || !names.has(name) || !args || (params && "task" in params)) return invalidParams();
    if (activeCalls.size >= MAX_CALLS) return error(res, id, -32000, "Too many active calls");
    clearTimeout(timer);
    timer = setTimeout(() => {
      controller.abort();
      error(res, id, -32000, "Tool call timed out");
    }, CALL_MS);
    // Keep the slot until the host really settles, even if it ignores cancellation.
    activeCalls.add(controller);
    try {
      const result = await onCall(id, name, args, meta, controller.signal);
      if (isLive()) success(res, id, encodeToolResult(result));
    } catch {
      if (isLive()) error(res, id, -32603, "Tool call failed");
    } finally {
      activeCalls.delete(controller);
    }
  }

  const server = createServer({ maxHeaderSize: 8192, headersTimeout: HTTP_MS, requestTimeout: HTTP_MS, connectionsCheckingInterval: 1000 },
    (req, res) => {
      const deadline = sockets.get(req.socket);
      if (deadline === undefined) { res.destroy(); return; }
      clearTimeout(deadline);
      sockets.set(req.socket, undefined);
      void handle(req, res).catch(() => res.destroy());
    });
  server.on("connection", socket => {
    if (closed || sockets.size >= MAX_CONNECTIONS) return socket.destroy();
    sockets.set(socket, setTimeout(() => socket.destroy(), HTTP_MS));
    socket.once("close", () => { clearTimeout(sockets.get(socket)); sockets.delete(socket); });
  });
  server.on("clientError", (_error, socket) => socket.destroy());
  server.on("checkContinue", (_req, res) => send(res, 417));
  server.setTimeout(CALL_MS + HTTP_MS, socket => socket.destroy());

  const abort = () => { void close(); };
  function close(): Promise<void> {
    if (closing) return closing;
    closed = true;
    signal.removeEventListener("abort", abort);
    closing = new Promise(resolve => server.close(() => resolve()));
    for (const controller of activeCalls) controller.abort();
    for (const socket of sockets.keys()) socket.destroy();
    return closing;
  }

  const started = await new Promise<boolean>(resolve => {
    const listening = () => resolve(true);
    server.once("listening", listening);
    server.once("close", () => { server.off("listening", listening); resolve(false); });
    server.on("error", () => { resolve(false); void close(); });
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) { resolve(false); void close(); return; }
    try { server.listen(0, ADDRESS); }
    catch { resolve(false); }
  });
  const address = started ? server.address() : null;
  if (closed || signal.aborted || !address || typeof address === "string") {
    await close();
    if (signal.aborted) throw aborted();
    throw new AcpFailure("launch", "could not start MCP bridge");
  }
  host = `${ADDRESS}:${address.port}`;
  return { descriptor: { type: "http", name: serverName, url: `http://${host}${path}`, headers: [] }, close };
}
