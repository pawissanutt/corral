// Fixture roles (host-bridge tests only): upstream read plus the corral hello, event and
// receipt_lookup operations. The control commands drive the fake transport from the bridge.
import { Type } from "typebox";

import type { Role, RoleModule } from "../../../src/roles/types.ts";

const TRANSPORT = Symbol.for("corral-pi.fixture.transport");
const FORBIDDEN_RUNS = Symbol.for("corral-pi.fixture.forbidden-runs");

export function forbiddenRuns(): number {
  return (globalThis as unknown as Record<symbol, number | undefined>)[FORBIDDEN_RUNS] ?? 0;
}

interface TransportControls {
  holdNext(): void;
  release(): void;
  failNext(): void;
  script(steps: readonly {
    text?: string;
    toolCalls?: readonly { callId: string; name: string; arguments: Record<string, unknown> }[];
  }[]): void;
}

function transport(): TransportControls {
  const value = (globalThis as unknown as Record<symbol, TransportControls | undefined>)[TRANSPORT];
  if (!value) throw new Error("the fixture auth module has not created its transport");
  return value;
}

export function fixtureRole(role: Role): RoleModule {
  return {
    role,
    upstreamTools: ["read"],
    corralOps: ["hello", "event", "receipt_lookup"],
    tools() {
      return [{
        name: "corral_receipt_lookup",
        label: "Look up a corral receipt",
        description: "Read the stored receipt of an earlier request of this container",
        op: "receipt_lookup",
        parameters: Type.Object({ request_id: Type.String() }),
        payload: (params: Record<string, unknown>) => ({ request_id: params.request_id }),
      }];
    },
    contract() {
      return `Fixture ${role} contract for host-bridge tests. Use only the tools you are given.`;
    },
    extend(pi) {
      pi.registerTool({
        name: "fixture_forbidden",
        label: "Registered outside the loadout",
        description: "A sentinel the host must never let run",
        parameters: Type.Object({}),
        async execute() {
          const holder = globalThis as unknown as Record<symbol, number | undefined>;
          holder[FORBIDDEN_RUNS] = (holder[FORBIDDEN_RUNS] ?? 0) + 1;
          return { content: [{ type: "text", text: "forbidden tool ran" }], details: {} };
        },
      });
      pi.registerCommand("fixture-handled", { description: "Handled without a model run", async handler() {} });
      pi.registerCommand("fixture-hold", { description: "Hold the next fake response", async handler() { transport().holdNext(); } });
      pi.registerCommand("fixture-release", { description: "Release the held fake response", async handler() { transport().release(); } });
      pi.registerCommand("fixture-fail", { description: "Fail the next fake request", async handler() { transport().failNext(); } });
      pi.registerCommand("fixture-call", {
        description: "Script one tool call by name",
        async handler(args) {
          const name = args.trim();
          transport().script([{ toolCalls: [{ callId: `call-${name}`, name, arguments: {} }] }, { text: "CALLED" }]);
        },
      });
      pi.registerCommand("fixture-reload", { description: "Reload resources", async handler(_args, context) { await context.reload(); } });
      pi.registerCommand("fixture-new-session", {
        description: "Replace the session",
        async handler(_args, context) { await context.newSession(); },
      });
    },
  };
}
