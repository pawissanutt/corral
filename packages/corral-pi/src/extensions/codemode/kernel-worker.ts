// The kernel worker (T-682): one thread per CodeKernel generation, started by kernel.ts with an
// empty environment. Cells run in a separate node:vm realm whose globals are the language
// intrinsics plus the bridge installed below: no Bun, process, require, fetch, worker timers or
// module loader (dynamic import is rejected). Values reach that realm only as JSON text or as
// objects it created, so this thread's objects are not reachable through `constructor` chains.
// This keeps capabilities on the Corral bridge; it is not an OS security boundary.
//
// Top-level declarations persist because Bun's REPL transform hoists them to globals of the
// realm. A tool call, output or timer belongs to the cell whose async context made it, so code
// left running by a settled cell can neither call tools nor print into a later cell.
import { AsyncLocalStorage } from "node:async_hooks";
import { randomBytes } from "node:crypto";
import vm from "node:vm";
import { parentPort } from "node:worker_threads";

import {
  BASE64, base64Bytes, type HostMessage, IMAGE_MIME_TYPES, type KernelOutput, OutputBudget, type WireError, type WorkerMessage,
  type WorkerStatus,
} from "./kernel-protocol.ts";

type Resolve = (json?: string) => void;
type Reject = (name: string, message: string, code?: string) => void;

interface Cell {
  readonly id: string;
  readonly budget: OutputBudget;
  readonly calls: Map<string, { readonly resolve: Resolve; readonly reject: Reject }>;
  readonly timers: Map<number, ReturnType<typeof setTimeout>>;
  next: number;
  settled: boolean;
}

interface Transpiler {
  transformSync(code: string): string;
}

const port = parentPort;
if (!port) throw new Error("kernel-worker runs only as a worker thread");
const scope = new AsyncLocalStorage<Cell>();
const TOOL_NAME = /^[\w.:-]{1,128}$/;
const END = "__corral_cell_end__";
const MODULE_RETURN = "Top-level return cannot be used inside an ECMAScript module";
const MAX_DELAY = 2_147_483_647;
const CANCELLED: WireError = { name: "AbortError", message: "the cell was cancelled", code: "cell_cancelled" };
const SETTLED: WireError = { name: "Error", message: "the cell had already finished", code: "cell_settled" };
const transpiler = new (Reflect.get(globalThis, "Bun") as { Transpiler: new (options: object) => Transpiler })
  .Transpiler({ loader: "ts", replMode: true });

// Runs inside the cell realm. `host` is the only reference into this thread and stays in the
// closure; every reserved global is read-only so a cell cannot replace the bridge for later cells.
const BOOTSTRAP = `"use strict";
(function install(host) {
  const { create, defineProperty, freeze, keys } = Object;
  const { parse, stringify } = JSON;
  const P = Promise, E = Error, TE = TypeError;
  const check = (result) => { if (typeof result === "string") throw new TE(result); return result; };
  const failure = (name, message, code) => {
    const error = new E(message);
    defineProperty(error, "name", { value: name, writable: true, configurable: true });
    if (code !== undefined) error.code = code;
    return error;
  };
  const call = (tool, input) => {
    let payload;
    try { payload = input === undefined ? undefined : stringify(input); }
    catch { return P.reject(new TE("tools." + tool + ": the input must be JSON-serializable")); }
    return new P((resolve, reject) => check(host("call", tool, payload,
      (json) => { try { resolve(json === undefined ? undefined : parse(json)); } catch (error) { reject(error); } },
      (name, message, code) => reject(failure(name, message, code)))));
  };
  const tools = new Proxy(freeze(create(null)), {
    get: (_target, key) => typeof key === "string" && key !== "then" ? (input) => call(key, input) : undefined,
  });
  const print = (...values) => { check(host("print", values)); };
  const display = (value) => { check(host("display", value)); };
  const end = freeze(create(null));
  const globals = {
    tools, print, display,
    console: freeze({ log: print, info: print, warn: print, error: print, debug: print }),
    setTimeout: (callback, delay, ...args) => {
      if (typeof callback !== "function") throw new TE("setTimeout needs a function");
      return check(host("setTimeout", () => callback(...args), delay));
    },
    clearTimeout: (id) => { check(host("clearTimeout", id)); },
    ${END}: end,
  };
  for (const name of keys(globals)) {
    defineProperty(globalThis, name, { value: globals[name], writable: false, enumerable: false, configurable: false });
  }
  defineProperty(E, "prepareStackTrace", { value: undefined, writable: false, configurable: false });
  return end;
})`;

const context = vm.createContext(Object.create(null), { name: "corral-code-kernel" });
const install = vm.runInContext(BOOTSTRAP, context) as (host: (...args: unknown[]) => unknown) => object;
const end = install(host);
let active: Cell | undefined;

function post(message: WorkerMessage): void {
  port?.postMessage(message);
}

function text(error: unknown, key: "name" | "message" | "code"): string | undefined {
  try {
    const value = (error as Record<string, unknown> | null | undefined)?.[key];
    return typeof value === "string" ? value : undefined;
  } catch {
    return undefined;
  }
}

function wireError(error: unknown, name = "Error"): WireError {
  const isObject = typeof error === "object" && error !== null;
  const code = isObject ? text(error, "code") : undefined;
  return {
    name: (isObject ? text(error, "name") : undefined) ?? name,
    message: (isObject ? text(error, "message") : undefined) ?? inspect(error),
    ...(code === undefined ? {} : { code }),
  };
}

function inspect(value: unknown): string {
  try {
    if (typeof value === "string") return value;
    if (value === undefined) return "undefined";
    if (typeof value === "function") return `[Function ${value.name || "anonymous"}]`;
    if (typeof value === "bigint") return `${value}n`;
    if (typeof value === "symbol") return value.toString();
    if (Object.prototype.toString.call(value) === "[object Error]") {
      return `${text(value, "name") ?? "Error"}: ${text(value, "message") ?? ""}`;
    }
    const seen = new WeakSet<object>();
    return JSON.stringify(value, (_key, item: unknown) => {
      if (typeof item === "bigint") return `${item}n`;
      if (typeof item === "function") return `[Function ${item.name || "anonymous"}]`;
      if (typeof item === "symbol" || item === undefined) return item === undefined ? undefined : item.toString();
      if (typeof item !== "object" || item === null) return item;
      if (seen.has(item)) return "[Circular]";
      seen.add(item);
      if (Object.prototype.toString.call(item) === "[object Map]") return [...(item as Map<unknown, unknown>)];
      if (Object.prototype.toString.call(item) === "[object Set]") return [...(item as Set<unknown>)];
      if (ArrayBuffer.isView(item)) return `[${item.constructor.name} ${item.byteLength} bytes]`;
      return item;
    }, 2) ?? String(value);
  } catch (error) {
    return `[unprintable value: ${text(error, "message") ?? "error"}]`;
  }
}

function emit(cell: Cell, item: KernelOutput): void {
  post({ op: "output", cell: cell.id, item });
}

function printed(values: unknown): string {
  if (!Array.isArray(values)) return inspect(values);
  const parts: string[] = [];
  for (let index = 0; index < values.length; index++) parts.push(inspect(values[index]));
  return parts.join(" ");
}

function display(cell: Cell, value: unknown): string | undefined {
  const fields = typeof value === "object" && value !== null ? value as Record<string, unknown> : undefined;
  if (fields?.type !== "image") {
    const admitted = cell.budget.text(inspect(value));
    if (admitted !== undefined) emit(cell, { type: "text", text: admitted });
    return undefined;
  }
  const { data, mimeType } = fields;
  if (typeof mimeType !== "string" || !IMAGE_MIME_TYPES.has(mimeType)) {
    return `display: image mimeType must be one of ${[...IMAGE_MIME_TYPES].join(", ")}`;
  }
  if (typeof data === "string") {
    if (!BASE64.test(data)) return "display: image data must be base64 text";
    if (cell.budget.image(base64Bytes(data), mimeType)) {
      emit(cell, { type: "image", data, mimeType });
    }
    return undefined;
  }
  if (!ArrayBuffer.isView(data)) return "display: image data must be base64 text or bytes";
  if (cell.budget.image(data.byteLength, mimeType)) {
    emit(cell, { type: "image", data: Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString("base64"), mimeType });
  }
  return undefined;
}

function startCall(cell: Cell | undefined, tool: unknown, input: unknown, resolve: unknown, reject: unknown): string | undefined {
  if (typeof resolve !== "function" || typeof reject !== "function") return "invalid tool call";
  if (typeof tool !== "string" || !TOOL_NAME.test(tool)) return "tool names are 1-128 letters, digits or _ . : -";
  if (input !== undefined && typeof input !== "string") return "invalid tool input";
  if (!cell) {
    (reject as Reject)(SETTLED.name, `${SETTLED.message}; tool calls must finish inside their cell`, SETTLED.code);
    return undefined;
  }
  const call = `${cell.id}.${++cell.next}`;
  cell.calls.set(call, { resolve: resolve as Resolve, reject: reject as Reject });
  post({ op: "call", cell: cell.id, call, tool, ...(input === undefined ? {} : { input }) });
  return undefined;
}

function schedule(cell: Cell, callback: () => unknown, delay: unknown): number {
  const id = ++cell.next;
  const ms = Math.min(Math.max(Number(delay) || 0, 0), MAX_DELAY);
  cell.timers.set(id, setTimeout(() => {
    cell.timers.delete(id);
    if (cell.settled) return;
    try {
      scope.run(cell, callback);
    } catch (error) {
      finish(cell, "error", { error: wireError(error) });
    }
  }, ms));
  return id;
}

// The single entry from the cell realm. It returns a problem string (thrown there as a
// TypeError) instead of throwing, so no error object of this thread reaches a cell.
function host(op: unknown, a?: unknown, b?: unknown, c?: unknown, d?: unknown): unknown {
  try {
    const store = scope.getStore();
    const cell = store && !store.settled ? store : undefined;
    switch (op) {
      case "call":
        return startCall(cell, a, b, c, d);
      case "print": {
        const admitted = cell?.budget.text(printed(a));
        if (cell && admitted !== undefined) emit(cell, { type: "text", text: admitted });
        return undefined;
      }
      case "display":
        return cell ? display(cell, a) : undefined;
      case "setTimeout":
        return cell && typeof a === "function" ? schedule(cell, a as () => unknown, b) : 0;
      case "clearTimeout":
        if (cell && typeof a === "number") {
          clearTimeout(cell.timers.get(a));
          cell.timers.delete(a);
        }
        return undefined;
      default:
        return `unknown kernel operation ${String(op)}`;
    }
  } catch (error) {
    return `kernel bridge failed: ${text(error, "message") ?? "unknown error"}`;
  }
}

function messages(error: unknown): string[] {
  const nested = (error as { errors?: unknown } | null)?.errors;
  return [error, ...(Array.isArray(nested) ? nested : [])].map((item) => text(item, "message") ?? "");
}

// Bun's REPL transform parses a cell that has top-level await as a module, where `return` is a
// syntax error. Each `return` keyword becomes `throw <marker>N, <expression>` (the same grammar:
// no line break after the keyword, one optional expression) and each marker is turned back into
// `return` inside the transform's async wrapper. Occurrences inside strings round-trip verbatim.
function transformReturns(source: string): string | undefined {
  const marker = `__corral_return_${randomBytes(6).toString("hex")}_`;
  const bare: boolean[] = [];
  const rewritten = source.replace(/(?<![\w$.])return(?![\w$])(?!\s*:)/g, (_match, offset: number) => {
    const lineEnd = source.indexOf("\n", offset);
    const isBare = /^[ \t]*(?:\r?$|[;}]|\/\/)/.test(source.slice(offset + 6, lineEnd < 0 ? undefined : lineEnd));
    bare.push(isBare);
    return `throw ${marker}${bare.length - 1},${isBare ? " void 0" : ""}`;
  });
  let output = transpiler.transformSync(rewritten);
  bare.forEach((isBare, index) => {
    output = output.replaceAll(`throw ${marker}${index},${isBare ? " void 0" : ""}`, "return");
  });
  return output.includes(marker) ? undefined : output;
}

function compile(code: string): vm.Script {
  const source = `"use strict";\n${code}\n;${END}`;
  let output: string;
  try {
    output = transpiler.transformSync(source);
  } catch (error) {
    const retried = messages(error).includes(MODULE_RETURN) ? transformReturns(source) : undefined;
    if (retried === undefined) throw error;
    output = retried;
  }
  return new vm.Script(output, { filename: "cell.js" });
}

function syntaxError(error: unknown): WireError {
  const nested = (error as { errors?: unknown } | null)?.errors;
  const first = Array.isArray(nested) && nested.length > 0 ? nested[0] : error;
  const position = (first as { position?: { line?: unknown; column?: unknown } } | null)?.position;
  const where = typeof position?.line === "number" && typeof position.column === "number"
    ? ` (line ${Math.max(1, position.line - 1)}, column ${position.column})` : "";
  return { name: "SyntaxError", message: `${text(first, "message") ?? "invalid cell code"}${where}` };
}

function returned(cell: Cell, completion: unknown): string | undefined {
  const fellThrough = typeof completion === "object" && completion !== null && Object.getPrototypeOf(completion) === null &&
    (completion as { value?: unknown }).value === end;
  return fellThrough || completion === undefined ? undefined : cell.budget.value(inspect(completion));
}

function settle(cell: Cell, reason: WireError): void {
  cell.settled = true;
  if (active === cell) active = undefined;
  for (const timer of cell.timers.values()) clearTimeout(timer);
  cell.timers.clear();
  const pending = [...cell.calls.values()];
  cell.calls.clear();
  for (const call of pending) call.reject(reason.name, reason.message, reason.code);
}

function finish(cell: Cell, status: WorkerStatus, result: { value?: string; error?: WireError }): void {
  if (cell.settled) return;
  settle(cell, status === "cancelled" ? CANCELLED : SETTLED);
  post({
    op: "done", cell: cell.id, status, truncated: cell.budget.truncated,
    ...(result.value === undefined ? {} : { value: result.value }), ...(result.error ? { error: result.error } : {}),
  });
}

async function run(message: Extract<HostMessage, { op: "run" }>): Promise<void> {
  const cell: Cell = {
    id: message.cell, budget: new OutputBudget(message.limits), calls: new Map(), timers: new Map(), next: 0, settled: false,
  };
  active = cell;
  let script: vm.Script;
  try {
    script = compile(message.code);
  } catch (error) {
    finish(cell, "error", { error: syntaxError(error) });
    return;
  }
  try {
    const completion: unknown = await scope.run(cell, () => script.runInContext(context));
    finish(cell, "ok", { value: returned(cell, completion) });
  } catch (error) {
    finish(cell, "error", { error: wireError(error) });
  }
}

function deliver(message: Extract<HostMessage, { op: "result" | "failure" }>): void {
  const cell = active;
  const pending = cell?.id === message.cell ? cell.calls.get(message.call) : undefined;
  if (!cell || !pending) return;
  cell.calls.delete(message.call);
  if (message.op === "result") pending.resolve(message.value);
  else pending.reject(message.error.name, message.error.message, message.error.code);
}

// A rejection nobody handled is reported to the running cell, like a REPL's "Uncaught (in
// promise)". Code of a settled cell has no output channel, so its rejections end here.
process.on("unhandledRejection", (reason) => {
  if (!active) return;
  const admitted = active.budget.text(`Uncaught (in promise) ${inspect(reason)}`);
  if (admitted !== undefined) emit(active, { type: "text", text: admitted });
});

port.on("message", (message: HostMessage) => {
  switch (message.op) {
    case "run":
      void run(message);
      break;
    case "cancel":
      if (active?.id === message.cell) finish(active, "cancelled", {});
      break;
    case "result":
    case "failure":
      deliver(message);
      break;
  }
});
post({ op: "ready", module: import.meta.url });
