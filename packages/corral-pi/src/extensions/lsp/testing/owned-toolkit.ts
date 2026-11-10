// Small real-executor probe for manager recovery. Red-test cleanup owns exact PID/start pairs.
import type { Lifecycle } from "../../../host/managed-effects.ts";
import { startTimeOf } from "../../jobs/runtime.ts";
import { lspToolkit, LspError, type LspLimits, type LspServerSpec } from "../index.ts";

export function ownedToolkit(root: string, spec: LspServerSpec, limits: Partial<LspLimits>, failFirstBinding = false) {
  const facts: Lifecycle[] = [];
  const owned = new Map<number, { started: string; exit: Promise<void> }>();
  let failedStartup = false;
  const toolkit = lspToolkit({ root, servers: [spec], limits,
    evidence: async event => {
      facts.push(event);
      if (failFirstBinding && event.action === "bind" && !failedStartup) {
        failedStartup = true;
        throw new LspError("lsp_request_failed", "fixture startup fails after binding");
      }
    },
    registerCapability: pid => {
      const started = startTimeOf(pid);
      if (!started) throw new Error("fixture child identity missing");
      const exited = Promise.withResolvers<void>();
      owned.set(pid, { started, exit: exited.promise });
      return () => exited.resolve();
    },
  });
  return { toolkit, facts, async cleanup(): Promise<void> {
    try { await toolkit.shutdown(); }
    finally {
      // Only our disposable registered children: never names, absent-PID guesses or inputs.
      for (const [pid, child] of owned) if (startTimeOf(pid) === child.started) process.kill(pid, "SIGKILL");
      await Promise.all([...owned.values()].map(child => child.exit));
    }
  } };
}
