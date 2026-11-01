// The persistent Code Mode kernel (T-682) on real worker threads. Every wait is an event barrier
// or a deferred the test resolves; the only deadlines are the kernel limits under test, applied
// to cells blocked in Atomics.wait (no CPU use) so they cannot acknowledge in time.
import { expect, test } from "bun:test";

import {
  type CellResult, CodeKernel, type CodeKernelOptions, CodeKernels, type KernelEvent, type ToolBridge,
} from "../src/extensions/codemode/kernel.ts";
import { within } from "./support/wait.ts";

const BLOCK = "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0);";
const echo: ToolBridge = async ({ input }) => input;

interface Deferred<T> {
  readonly promise: Promise<T>;
  resolve(value: T): void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => { resolve = settle; });
  return { promise, resolve: (value) => resolve(value) };
}

function harness(options: Partial<CodeKernelOptions> = {}) {
  const events: KernelEvent[] = [];
  const listeners = new Set<(event: KernelEvent) => void>();
  const kernel = new CodeKernel({
    session: "session-a", tools: echo, ...options,
    onEvent: (event) => {
      events.push(event);
      for (const listener of [...listeners]) listener(event);
    },
  });
  /** Subscribe first, then act: resolves on the first matching event after this call. */
  const next = (match: (event: KernelEvent) => boolean, what = "a kernel event"): Promise<KernelEvent> => {
    let listener: ((event: KernelEvent) => void) | undefined;
    const seen = new Promise<KernelEvent>((resolve) => {
      listener = (event) => { if (match(event)) resolve(event); };
      listeners.add(listener);
    });
    return within(seen, 10_000, what).finally(() => { if (listener) listeners.delete(listener); });
  };
  return { kernel, events, next };
}

const json = (result: CellResult): unknown => JSON.parse(result.value ?? "null");
const cellOf = (event: KernelEvent): string | undefined => "cell" in event ? event.cell : undefined;
const callOf = (event: KernelEvent): string | undefined => "call" in event ? event.call : undefined;

test("top-level variables, functions and classes persist across cells; top-level await and return work", async () => {
  const { kernel } = harness();
  try {
    const first = await kernel.submit(
      'const data = await tools.echo({ n: 2 }); function twice(x) { return x * 2 } class Box { constructor(v) { this.v = v } }' +
      ' let label = "kept"; data.n + 1',
    ).result;
    expect(first).toMatchObject({ status: "ok", generation: 1, stateLost: false, outputs: [] });
    expect(first.value).toBeUndefined();
    const second = await kernel.submit('const doubled = twice(data.n); return [doubled, new Box(label).v, await Promise.resolve("awaited")];').result;
    expect(json(second)).toEqual([4, "kept", "awaited"]);
    const early = await kernel.submit(
      'await null; try { if (doubled === 4) return { early: true, text: "a return inside a string" } } finally { label = "finally ran" }\nreturn "late"',
    ).result;
    expect(json(early)).toEqual({ early: true, text: "a return inside a string" });
    const nested = await kernel.submit("await null; const pick = (x) => { return x.label }; return pick({ label })").result;
    expect(nested.value).toBe("finally ran");
    expect((await kernel.submit("await null; if (label) return\nreturn 'unreachable'").result).value).toBeUndefined();
  } finally {
    await kernel.close();
  }
});

test("each bound session has its own kernel; a closed session starts empty without replay", async () => {
  let echoes = 0;
  const kernels = new CodeKernels((session) => new CodeKernel({ session, tools: async ({ input }) => { echoes++; return input; } }));
  try {
    const a = kernels.kernel("session-a");
    expect(kernels.kernel("session-a")).toBe(a);
    const b = kernels.kernel("session-b");
    expect(b).not.toBe(a);
    await a.submit('globalThis.shared = "a"; let secret = await tools.echo(1);').result;
    expect(json(await b.submit("return [typeof shared, typeof secret]").result)).toEqual(["undefined", "undefined"]);
    expect(json(await a.submit("return [shared, secret]").result)).toEqual(["a", 1]);
    await kernels.close("session-a");
    const again = kernels.kernel("session-a");
    expect(again).not.toBe(a);
    const fresh = await again.submit("return typeof secret").result;
    expect(fresh).toMatchObject({ value: "undefined", generation: 1 });
    expect(echoes).toBe(1);
  } finally {
    await kernels.closeAll();
  }
});

test("cells run one at a time in submission order", async () => {
  const gate = deferred<unknown>();
  const { kernel, events, next } = harness({ tools: async () => gate.promise });
  try {
    const blocked = next((event) => event.kind === "call_started");
    const first = kernel.submit('let order = ["first"]; await tools.wait({}); order.push("first done");');
    const second = kernel.submit('order.push("second"); return order;');
    const third = kernel.submit('order.push("third"); return order.length;');
    await blocked;
    expect(kernel.status()).toMatchObject({ running: first.cell, queued: [second.cell, third.cell] });
    gate.resolve({});
    expect(json(await second.result)).toEqual(["first", "first done", "second"]);
    expect((await third.result).value).toBe("4");
    expect((await first.result).status).toBe("ok");
    const lifecycle = events.filter((event) => event.kind === "cell_started" || event.kind === "cell_finished")
      .map((event) => `${event.kind}:${cellOf(event)}`);
    expect(lifecycle).toEqual([first, second, third].flatMap(({ cell }) => [`cell_started:${cell}`, `cell_finished:${cell}`]));
  } finally {
    await kernel.close();
  }
});

test("independent calls overlap; results and errors reach the cell; only explicit output is returned", async () => {
  const running = new Set<string>();
  const both = deferred<void>();
  const large = "x".repeat(200_000);
  const tools: ToolBridge = async ({ tool, call }) => {
    running.add(call);
    if (running.size === 2) both.resolve();
    await both.promise;
    if (tool === "deny") throw Object.assign(new Error("deny is not in this session's loadout"), { code: "tool_denied" });
    return { tool, body: large };
  };
  const { kernel, events } = harness({ tools });
  try {
    const handle = kernel.submit(`
      const [a, b] = await Promise.all([tools.read({ path: "a" }), tools.read({ path: "b" })]);
      let denied;
      try { await tools.deny({}); } catch (error) { denied = [error instanceof Error, error.name, error.message, error.code]; }
      print("bytes", a.body.length + b.body.length);
      return denied;`);
    const result = await handle.result;
    expect(result.status).toBe("ok");
    expect(json(result)).toEqual([true, "Error", "deny is not in this session's loadout", "tool_denied"]);
    expect(result.outputs).toEqual([{ type: "text", text: "bytes 400000" }]);
    expect(JSON.stringify(result).length < 1000).toBeTrue();
    const calls = events.filter((event) => event.kind.startsWith("call_"));
    expect(calls.every((event) => cellOf(event) === handle.cell)).toBeTrue();
    const ids = [...new Set(calls.map(callOf))];
    expect(ids).toHaveLength(3);
    const starts = calls.filter((event) => event.kind === "call_started").map((event) => "tool" in event ? event.tool : "");
    expect(starts).toEqual(["read", "read", "deny"]);
    const firstFinish = calls.findIndex((event) => event.kind === "call_finished");
    expect(calls.slice(0, firstFinish).filter((event) => event.kind === "call_started")).toHaveLength(2);
    const finished = calls.filter((event) => event.kind === "call_finished").map((event) => "ok" in event ? event.ok : undefined);
    expect(finished).toEqual([true, true, false]);
    expect(events.at(-1)).toMatchObject({ kind: "cell_finished", cell: handle.cell, status: "ok" });
  } finally {
    await kernel.close();
  }
});

test("a queued cell is cancelled without ever running", async () => {
  const gate = deferred<unknown>();
  const { kernel, events, next } = harness({ tools: async () => gate.promise });
  try {
    const blocked = next((event) => event.kind === "call_started");
    const first = kernel.submit('await tools.wait({}); return "first";');
    const queued = kernel.submit('globalThis.ran = true; return "queued";');
    await blocked;
    expect(kernel.cancel(queued.cell)).toBeTrue();
    expect(await queued.result).toMatchObject({ status: "cancelled", stateLost: false, outputs: [] });
    gate.resolve({});
    expect((await first.result).value).toBe("first");
    expect((await kernel.submit("return typeof ran").result).value).toBe("undefined");
    expect(events.some((event) => event.kind === "cell_started" && cellOf(event) === queued.cell)).toBeFalse();
    expect(kernel.cancel(queued.cell)).toBeFalse();
  } finally {
    await kernel.close();
  }
});

test("cancelling an active cell aborts its call, discards the late result and keeps the kernel state", async () => {
  let signal: AbortSignal | undefined;
  const late = deferred<unknown>();
  const { kernel, next } = harness({
    tools: async (request) => {
      if (request.tool !== "hold") return request.input;
      signal = request.signal;
      return late.promise;
    },
  });
  try {
    await kernel.submit("let kept = 41;").result;
    const holding = next((event) => event.kind === "call_started");
    const active = kernel.submit("await tools.hold({}); kept = 0;");
    await holding;
    expect(kernel.cancel(active.cell)).toBeTrue();
    expect(await active.result).toMatchObject({ status: "cancelled", stateLost: false, generation: 1 });
    expect(signal?.aborted).toBeTrue();
    const discarded = next((event) => event.kind === "call_discarded" && cellOf(event) === active.cell);
    late.resolve("too late");
    await discarded;
    expect(await kernel.submit("return kept + 1").result).toMatchObject({ status: "ok", value: "42", generation: 1 });
  } finally {
    await kernel.close();
  }
});

test("a cell that cannot be interrupted is replaced by a fresh kernel and the state loss is reported", async () => {
  const { kernel, next } = harness({ limits: { cancelGraceMs: 50 } });
  try {
    await kernel.submit("let before = 1;").result;
    const started = next((event) => event.kind === "cell_started" && cellOf(event) === "stuck");
    const stuck = kernel.submit(BLOCK, { cell: "stuck" });
    const queued = kernel.submit("return typeof before;");
    await started;
    const lost = next((event) => event.kind === "state_lost");
    kernel.cancel("stuck");
    expect(await lost).toMatchObject({ kind: "state_lost", reason: "unresponsive", generation: 1, cells: 2 });
    expect(await stuck.result).toMatchObject({
      status: "cancelled", stateLost: true, generation: 1, error: { code: "kernel_restarted" },
    });
    expect(await queued.result).toMatchObject({ status: "ok", value: "undefined", generation: 2, stateLost: false });
  } finally {
    await kernel.close();
  }
});

test("execution is bounded: a deadline stops a waiting cell in place and replaces a blocked one", async () => {
  const { kernel, next } = harness({ tools: () => new Promise(() => undefined), limits: { cancelGraceMs: 50 } });
  try {
    await kernel.submit("let kept = 'yes';").result;
    const waiting = await kernel.submit("await tools.never({});", { timeoutMs: 50 }).result;
    expect(waiting).toMatchObject({ status: "timeout", stateLost: false, generation: 1 });
    expect((await kernel.submit("return kept").result).value).toBe("yes");
    const lost = next((event) => event.kind === "state_lost");
    const blocked = await kernel.submit(BLOCK, { timeoutMs: 50 }).result;
    expect(blocked).toMatchObject({ status: "timeout", stateLost: true, generation: 1 });
    expect(await lost).toMatchObject({ reason: "unresponsive" });
    expect(await kernel.submit("return typeof kept").result).toMatchObject({ value: "undefined", generation: 2 });
  } finally {
    await kernel.close();
  }
});

test("reset with a pending call reports state loss, never replays and never lets the late result settle a new cell", async () => {
  const calls = new Map<string, { readonly answer: Deferred<unknown>; readonly signal: AbortSignal }>();
  let executions = 0;
  const { kernel, next } = harness({
    tools: async ({ tool, call, signal }) => {
      if (tool === "count") return ++executions;
      const answer = deferred<unknown>();
      calls.set(call, { answer, signal });
      return answer.promise;
    },
  });
  try {
    await kernel.submit("let state = await tools.count({});").result;
    const firstCall = next((event) => event.kind === "call_started" && "tool" in event && event.tool === "slow");
    const pending = kernel.submit("return await tools.slow({ n: 1 });");
    const first = callOf(await firstCall) ?? "";
    const lost = next((event) => event.kind === "state_lost");
    await kernel.reset();
    expect(await lost).toMatchObject({ reason: "reset", generation: 1, cells: 2 });
    expect(await pending.result).toMatchObject({ status: "reset", stateLost: true, generation: 1 });
    expect(calls.get(first)?.signal.aborted).toBeTrue();
    const secondCall = next((event) => event.kind === "call_started");
    const fresh = kernel.submit("return [typeof state, await tools.slow({ n: 2 })];");
    const second = callOf(await secondCall) ?? "";
    const discarded = next((event) => event.kind === "call_discarded" && callOf(event) === first);
    calls.get(first)?.answer.resolve("late from generation 1");
    await discarded;
    calls.get(second)?.answer.resolve("fresh");
    const result = await fresh.result;
    expect(result).toMatchObject({ status: "ok", generation: 2, stateLost: false });
    expect(json(result)).toEqual(["undefined", "fresh"]);
    expect(executions).toBe(1);
  } finally {
    await kernel.close();
  }
});

test("text, image and return-value output is bounded per cell", async () => {
  const { kernel } = harness({ limits: { output: { maxTextBytes: 32, maxValueBytes: 8, maxImages: 2, maxImageBytes: 16 } } });
  const image = Buffer.from("sixteen bytes!!!").toString("base64");
  try {
    const text = await kernel.submit('for (let i = 0; i < 10; i++) console.log("line", i); return "0123456789abcdef";').result;
    expect(text.truncated).toBeTrue();
    const lines = text.outputs.map((item) => item.type === "text" ? item.text : "");
    expect(lines.slice(0, 2)).toEqual(["line 0", "line 1"]);
    expect(Buffer.byteLength(lines.join(""))).toBe(32);
    expect(text.value).toBe("01234567");
    const images = await kernel.submit(`
      const image = { type: "image", data: "${image}", mimeType: "image/png" };
      display(image);
      display({ type: "image", data: new Uint8Array(17), mimeType: "image/png" });
      display({ type: "image", data: new Uint8Array(16), mimeType: "image/webp" });
      display(image);
      try { display({ ...image, mimeType: "text/html" }); } catch (error) { return error instanceof TypeError; }`).result;
    expect(images.truncated).toBeTrue();
    expect(images.value).toBe("true");
    expect(images.outputs).toEqual([
      { type: "image", data: image, mimeType: "image/png" },
      { type: "image", data: Buffer.alloc(16).toString("base64"), mimeType: "image/webp" },
    ]);
    const fresh = await kernel.submit('print("budgets are per cell"); return 1').result;
    expect(fresh).toMatchObject({ truncated: false, value: "1", outputs: [{ type: "text", text: "budgets are per cell" }] });
  } finally {
    await kernel.close();
  }
});

test("close settles live cells, aborts calls, terminates the worker and refuses later work", async () => {
  let signal: AbortSignal | undefined;
  const { kernel, events, next } = harness({
    tools: (request) => {
      signal = request.signal;
      return new Promise(() => undefined);
    },
    limits: { exitWaitMs: 30_000 },
  });
  const holding = next((event) => event.kind === "call_started");
  const active = kernel.submit("await tools.hold({});");
  const queued = kernel.submit("return 1;");
  await holding;
  await kernel.close();
  expect(await active.result).toMatchObject({ status: "closed", stateLost: true });
  expect(await queued.result).toMatchObject({ status: "closed", stateLost: false });
  expect(signal?.aborted).toBeTrue();
  expect(kernel.status()).toMatchObject({ worker: false, retiring: 0, closed: true, running: undefined, queued: [] });
  expect(events.at(-1)).toMatchObject({ kind: "kernel_closed", cells: 1 });
  expect(() => kernel.submit("return 1")).toThrow(/kernel_closed/);
  await expect(kernel.reset()).rejects.toThrow(/kernel_closed/);
  await kernel.close();
});

test("cells see no ambient runtime, module loader or host-realm objects", async () => {
  const { kernel } = harness({
    tools: async ({ tool, input }) => {
      if (tool === "fail") throw new Error("refused");
      return input;
    },
  });
  try {
    const probes = await kernel.submit(`
      const probes = {};
      probes.globals = [typeof Bun, typeof process, typeof fetch, typeof WebSocket, typeof Worker, typeof globalThis.require,
        typeof module, typeof queueMicrotask];
      try { require("node:fs"); probes.require = "loaded"; } catch (error) { probes.require = error.name; }
      try { await import("node:fs"); probes.import = "loaded"; } catch (error) { probes.import = error.code; }
      try { await eval('import("node:child_process")'); probes.evalImport = "loaded"; } catch (error) { probes.evalImport = error.code; }
      const result = await tools.echo({});
      let failure;
      try { await tools.fail({}); } catch (error) { failure = error; }
      probes.escapes = [tools.echo, this.constructor, result.constructor, failure.constructor, print, setTimeout]
        .map((value) => value.constructor("return typeof process")());
      try { tools = null; probes.readonly = "replaced"; } catch (error) { probes.readonly = error.name; }
      return probes;`).result;
    expect(json(probes)).toEqual({
      globals: Array(8).fill("undefined"), require: "ReferenceError", import: "ERR_VM_DYNAMIC_IMPORT_CALLBACK_MISSING",
      evalImport: "ERR_VM_DYNAMIC_IMPORT_CALLBACK_MISSING", escapes: Array(6).fill("undefined"), readonly: "TypeError",
    });
    const staticImport = await kernel.submit('import { readFileSync } from "node:fs"; return typeof readFileSync;').result;
    expect(staticImport).toMatchObject({ status: "error", error: { code: "ERR_VM_DYNAMIC_IMPORT_CALLBACK_MISSING" } });
  } finally {
    await kernel.close();
  }
});

test("errors and abandoned timers stay inside their cell and the kernel stays usable", async () => {
  const { kernel } = harness();
  try {
    await kernel.submit("let count = 1;").result;
    expect(await kernel.submit("let = = 1").result).toMatchObject({ status: "error", error: { name: "SyntaxError" } });
    expect(await kernel.submit('count++; throw new RangeError("bad");').result)
      .toMatchObject({ status: "error", error: { name: "RangeError", message: "bad" } });
    expect(await kernel.submit('setTimeout(() => { throw new Error("timer failed"); }, 0); await new Promise(() => undefined);').result)
      .toMatchObject({ status: "error", error: { message: "timer failed" } });
    expect((await kernel.submit('setTimeout(() => print("left behind"), 0); return "settled";').result).outputs).toEqual([]);
    // Equal-delay timers fire in creation order, so the abandoned timer is due before this one.
    const later = await kernel.submit('await new Promise((resolve) => setTimeout(resolve, 0)); return count;').result;
    expect(later).toMatchObject({ status: "ok", value: "2", outputs: [] });
  } finally {
    await kernel.close();
  }
});

test("a worker that fails to start fails its cell with a state-loss report and the kernel keeps serving", async () => {
  const { kernel, next } = harness({ worker: new URL("./fixtures/missing-kernel-worker.ts", import.meta.url) });
  try {
    const lost = next((event) => event.kind === "state_lost");
    const result = await kernel.submit("return 1").result;
    expect(result).toMatchObject({ status: "error", stateLost: true, generation: 1, error: { code: "worker_failed" } });
    expect(await lost).toMatchObject({ reason: "worker_failed", cells: 1 });
    expect(await kernel.submit("return 2").result).toMatchObject({ status: "error", generation: 2 });
  } finally {
    await kernel.close();
  }
});

test("submission limits are enforced at the boundary", async () => {
  const gate = deferred<unknown>();
  const { kernel, next } = harness({ tools: async () => gate.promise, limits: { maxQueuedCells: 1, maxCodeBytes: 64 } });
  try {
    const blocked = next((event) => event.kind === "call_started");
    const first = kernel.submit("await tools.wait({});", { cell: "one" });
    await blocked;
    expect(() => kernel.submit("return 1", { cell: "one" })).toThrow(/invalid_cell/);
    expect(() => kernel.submit("return 1", { cell: "bad id!" })).toThrow(/invalid_cell/);
    expect(() => kernel.submit("return 1", { timeoutMs: 0 })).toThrow(/invalid_cell/);
    expect(() => kernel.submit(`return "${"x".repeat(64)}"`)).toThrow(/code_too_large/);
    const queued = kernel.submit("return 2");
    expect(() => kernel.submit("return 3")).toThrow(/queue_full/);
    gate.resolve({});
    expect((await first.result).status).toBe("ok");
    expect((await queued.result).value).toBe("2");
  } finally {
    await kernel.close();
  }
});
