// Non-yielding cells (a spin loop, a microtask loop) keep a CPU busy until their worker is
// terminated, so this file runs them only inside the capped container sandbox (AGENTS.md):
//   scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- \
//     bash -c 'cd packages/corral-pi && CORRAL_LOAD_SANDBOX=1 bun test test/code-kernel-load.test.ts'
// Anywhere else it skips with a printed note.
import { existsSync } from "node:fs";
import { expect, test } from "bun:test";

import { CodeKernel, type KernelEvent } from "../src/extensions/codemode/kernel.ts";
import { within } from "./support/wait.ts";

const SANDBOXED = process.env.CORRAL_LOAD_SANDBOX === "1" && existsSync("/run/.containerenv");

const SPINS = {
  "a synchronous spin loop": "while (true) {}",
  "a microtask loop": "for (;;) await null;",
  "a spin after a tool call": "await tools.echo({}); for (;;) {}",
} as const;

for (const [name, code] of Object.entries(SPINS)) {
  test(`${name} cannot freeze the host; cancel and timeout replace the kernel`, async () => {
    if (!SANDBOXED) {
      console.log(`skipped: non-yielding cells run only inside scripts/load-sandbox.sh with CORRAL_LOAD_SANDBOX=1`);
      return;
    }
    const events: KernelEvent[] = [];
    let onEvent: ((event: KernelEvent) => void) | undefined;
    const kernel = new CodeKernel({
      session: "load", tools: async ({ input }) => input, limits: { cancelGraceMs: 200, exitWaitMs: 10_000 },
      onEvent: (event) => { events.push(event); onEvent?.(event); },
    });
    try {
      await kernel.submit("let kept = 1;").result;
      const started = within(new Promise<void>((resolve) => {
        onEvent = (event) => { if (event.kind === "cell_started" && event.cell === "spin") resolve(); };
      }), 10_000, "the spinning cell to start");
      const spinning = kernel.submit(code, { cell: "spin" });
      await started;
      // The host's event loop still turns while the worker spins.
      await within(new Promise((resolve) => setTimeout(resolve, 0)), 10_000, "a host timer during the spin");
      expect(kernel.status().running).toBe("spin");
      kernel.cancel("spin");
      expect(await spinning.result).toMatchObject({ status: "cancelled", stateLost: true, generation: 1 });
      expect(events.some((event) => event.kind === "state_lost" && event.reason === "unresponsive")).toBeTrue();
      await kernel.reset();
      expect(kernel.status().retiring).toBe(0);
      expect(await kernel.submit(code, { timeoutMs: 100 }).result).toMatchObject({ status: "timeout", stateLost: true });
      await kernel.reset();
      expect(kernel.status().retiring).toBe(0);
      expect(await kernel.submit("return typeof kept").result).toMatchObject({ status: "ok", value: "undefined" });
    } finally {
      await kernel.close();
    }
    expect(kernel.status()).toMatchObject({ worker: false, retiring: 0 });
  }, 60_000);
}
