import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { ProbeFailure } from "./credentials.mjs";

/**
 * @typedef {{
 * generation: number, sessionId: string, forbiddenExecutions: number, mutations: number,
 * approved: boolean, questionPending: boolean,
 * context?: import("./output/runtime/node_modules/@earendil-works/pi-coding-agent/dist/index.js").ExtensionContext
 * }} FixtureState
 */

/**
 * @param {{state: FixtureState, root: string,
 * emit: (value: Record<string, unknown>) => void,
 * Type: typeof import("./output/runtime/node_modules/typebox/build/typebox.mjs")}} options
 * @returns {import("./output/runtime/node_modules/@earendil-works/pi-coding-agent/dist/index.js").ExtensionFactory}
 */
export function fixtureExtension(options) {
  const { state, emit, root, Type } = options;
  return (pi) => {
    pi.on("session_start", (_event, context) => {
      state.context = context;
      emit({ event: "native_session_bound", mode: context.mode, hasUI: context.hasUI,
        nativeSessionId: context.sessionManager.getSessionId() });
    });
    pi.on("tool_call", (event) => {
      if (!["pm_readback", "pm_fixture_mutate"].includes(event.toolName)) {
        emit({ event: "tool_denied", tool: event.toolName });
        return { block: true, reason: "outside disposable PM allowlist" };
      }
      return undefined;
    });
    pi.on("user_bash", () => ({
      result: { output: "Disabled in the disposable PM fixture", exitCode: 1, cancelled: false, truncated: false },
    }));
    pi.registerTool({
      name: "forbidden_deferred", label: "Forbidden fixture sentinel",
      description: "Never allowed", exposure: "deferred", parameters: Type.Object({}),
      async execute() {
        state.forbiddenExecutions++;
        throw new ProbeFailure("forbidden_tool_executed");
      },
    });
    pi.registerTool({
      name: "pm_fixture_mutate", label: "Fixture-only mutation",
      description: "Mutates only the private disposable fixture after an exact terminal decision",
      exposure: "deferred",
      parameters: Type.Object({ decision: Type.String(), revision: Type.String() }),
      async execute(_id, input) {
        if (!state.approved || state.mutations !== 0 ||
            input.decision !== "fixture-decision-1" || input.revision !== "rev1") {
          throw new ProbeFailure("fixture_exact_action_not_approved");
        }
        state.approved = false;
        state.mutations++;
        writeFileSync(join(root, "fixture-mutation.json"), JSON.stringify({
          mutations: state.mutations, decision: input.decision, revision: input.revision,
        }) + "\n", { mode: 0o600 });
        emit({ event: "fixture_committed", mutations: state.mutations,
          decision: input.decision, revision: input.revision });
        return { content: [{ type: "text", text: "Private fixture changed" }],
          details: { mutations: state.mutations } };
      },
    });
    pi.registerTool({
      name: "pm_readback", label: "Bounded fixture PM readback",
      description: "Read only the private fixture; no project or account access",
      parameters: Type.Object({ operation: Type.Union([
        Type.Literal("read"), Type.Literal("denials"), Type.Literal("mutate"),
      ]) }),
      async execute(_id, input, _signal, _update, context) {
        if (input.operation === "mutate") {
          const result = await context.executeTool("pm_fixture_mutate", {
            decision: "fixture-decision-1", revision: "rev1",
          });
          const errors = result.result.content.filter((item) => item.type === "text").map((item) => item.text).join("\n");
          emit({ event: "fixture_nested_result", isError: result.isError,
            callableTools: context.tools.map((tool) => tool.name),
            reason: !result.isError ? "fixture_success" :
              errors.includes("No assistant message") ? "missing_assistant_parent" :
              /not found|unknown tool/i.test(errors) ? "tool_not_callable" :
              errors.includes("fixture_exact_action_not_approved") ? "exact_action_not_approved" :
              "unclassified_fixture_tool_failure" });
          return { content: [{ type: "text", text: "Fixture action result" }],
            details: { isError: result.isError, mutations: state.mutations } };
        }
        /** @type {Array<{tool: string, denied: boolean}>} */
        const denials = [];
        let nestedReadSucceeded = false;
        if (input.operation === "denials") {
          const baseline = await context.executeTool("pm_readback", { operation: "read" });
          nestedReadSucceeded = !baseline.isError;
          for (const tool of ["bash", "read", "write", "edit", "codemode", "tool_search", "forbidden_deferred"]) {
            const result = await context.executeTool(tool, {});
            denials.push({ tool, denied: result.isError });
          }
        }
        return { content: [{ type: "text", text: "Private fixture readback" }],
          details: { denials, nestedReadSucceeded, mutations: state.mutations, forbiddenExecutions: state.forbiddenExecutions } };
      },
    });
    pi.registerCommand("probe-handled", {
      description: "Handle fixture input without model work",
      async handler() { emit({ event: "input_handled" }); },
    });
    pi.registerCommand("probe-reload", {
      description: "Reload only explicit disposable resources",
      async handler(_args, context) { await context.reload(); },
    });
    pi.registerCommand("probe-decision", {
      description: "Test trusted terminal input for one exact private-fixture action",
      async handler(_args, context) {
        state.questionPending = true;
        emit({ event: "awaiting_fixture_owner", decision: "fixture-decision-1", revision: "rev1" });
        const pendingAnswer = context.ui.input("Fixture only: approve increment 0 -> 1",
          "approve fixture-decision-1 rev1");
        // Pinned showExtensionInput installs/focuses the native component
        // synchronously. Generic provider ui_prompt events do not cover it.
        emit({ event: "fixture_dialog_open" });
        const answer = await pendingAnswer;
        state.approved = answer === "approve fixture-decision-1 rev1";
        state.questionPending = !state.approved;
        emit({ event: "fixture_decision_answered", approved: state.approved,
          decision: "fixture-decision-1", revision: "rev1", actor: "simulated_fixture_terminal" });
      },
    });
  };
}
