// Offline diagnostic for the persistent Code Mode kernel (T-682), independent of model
// credentials: a fixture bridge drives one kernel through persistence, overlapping bridge calls,
// queued and active cancellation, reset, recovery from a cell that cannot be interrupted, and
// close. It prints one line per proof and throws on the first mismatch.
import { type CellResult, CodeKernel, type KernelEvent, type ToolBridge } from "../extensions/codemode/kernel.ts";

type Expectation = (result: CellResult) => boolean;

export async function checkCodeKernel(): Promise<void> {
  const listeners = new Set<(event: KernelEvent) => void>();
  const events: KernelEvent[] = [];
  const next = (match: (event: KernelEvent) => boolean): Promise<KernelEvent> => new Promise((resolve) => {
    const listener = (event: KernelEvent) => {
      if (!match(event)) return;
      listeners.delete(listener);
      resolve(event);
    };
    listeners.add(listener);
  });
  const tools: ToolBridge = async ({ tool, input, signal }) => {
    if (tool === "echo") return input;
    if (tool === "hold") {
      return new Promise((_resolve, reject) => signal.addEventListener("abort", () => reject(new Error("released"))));
    }
    throw new Error(`unknown tool ${tool}`);
  };
  const kernel = new CodeKernel({
    session: "code-kernel-check", tools, limits: { cancelGraceMs: 200 },
    onEvent: (event) => {
      events.push(event);
      for (const listener of [...listeners]) listener(event);
    },
  });
  const started = next((event) => event.kind === "kernel_started");
  const expect = async (name: string, result: Promise<CellResult>, ok: Expectation) => {
    const settled = await result;
    if (!ok(settled)) throw new Error(`code kernel ${name} failed: ${JSON.stringify(settled)}`);
    process.stdout.write(`code kernel ${name}: ${JSON.stringify({ status: settled.status, generation: settled.generation,
      stateLost: settled.stateLost, value: settled.value })}\n`);
  };
  try {
    await expect("first cell", kernel.submit(
      'const greeting = await tools.echo({ text: "persist" }); let count = 1; function bump() { return ++count; } return greeting.text;',
    ).result, (r) => r.status === "ok" && r.value === "persist");
    const event = await started;
    if (event.kind !== "kernel_started" || !/\$bunfs|~BUN|%7EBUN/.test(event.module)) {
      throw new Error(`code kernel worker was not loaded from the executable: ${JSON.stringify(event)}`);
    }
    process.stdout.write(`code kernel worker: ${event.module}\n`);
    await expect("second cell", kernel.submit(
      'const [a, b] = await Promise.all([tools.echo({ text: "a" }), tools.echo({ text: "b" })]);' +
      " return { greeting: greeting.text, count: bump(), parallel: a.text + b.text };",
    ).result, (r) => r.status === "ok" && r.value !== undefined &&
      JSON.stringify(JSON.parse(r.value)) === '{"greeting":"persist","count":2,"parallel":"ab"}');

    const holding = next((e) => e.kind === "call_started" && e.tool === "hold");
    const active = kernel.submit("await tools.hold({});");
    const queued = kernel.submit('return "never runs";');
    await holding;
    kernel.cancel(queued.cell);
    kernel.cancel(active.cell);
    await expect("queued cancel", queued.result, (r) => r.status === "cancelled" && !r.stateLost);
    await expect("active cancel", active.result, (r) => r.status === "cancelled" && !r.stateLost);
    await expect("state after cancel", kernel.submit("return bump();").result, (r) => r.value === "3");

    const resetLoss = next((e) => e.kind === "state_lost" && e.reason === "reset");
    await kernel.reset();
    await resetLoss;
    await expect("after reset", kernel.submit("let survivor = 1; return typeof greeting;").result,
      (r) => r.value === "undefined" && r.generation === 2);

    const blocked = next((e) => e.kind === "cell_started");
    const stuck = kernel.submit("Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0);");
    await blocked;
    const recovered = next((e) => e.kind === "state_lost" && e.reason === "unresponsive");
    kernel.cancel(stuck.cell);
    await recovered;
    await expect("unresponsive cancel", stuck.result, (r) => r.status === "cancelled" && r.stateLost);
    await expect("after recovery", kernel.submit("return typeof survivor;").result,
      (r) => r.value === "undefined" && r.generation === 3);
  } finally {
    await kernel.close();
  }
  const status = kernel.status();
  if (status.worker || status.retiring !== 0 || !events.some((event) => event.kind === "kernel_closed")) {
    throw new Error(`code kernel close left work behind: ${JSON.stringify(status)}`);
  }
  process.stdout.write(`code kernel closed: ${JSON.stringify(status)}\ncode kernel check passed\n`);
}
