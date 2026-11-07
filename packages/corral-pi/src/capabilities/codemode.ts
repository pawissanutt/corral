// The `codemode` tool (T-683): cells of JavaScript on this session's Corral-owned persistent Bun
// kernel (extensions/codemode). It is the only code tool; no QuickJS or other engine remains. The
// callable list in its description and every nested execution derive from one predicate, the
// host's execution check, so code reaches exactly the tools a direct call could.
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { type CellReport, reportContent } from "../extensions/codemode/report.ts";
import { type CodeRuntime, DEFAULT_WAIT_MS, MAX_WAIT_MS } from "../extensions/codemode/runtime.ts";

export const CODEMODE_TOOL = "codemode";

const DESCRIPTION = [
  "Run a JavaScript cell in this session's persistent Corral kernel to batch, chain and filter tool calls.",
  "Top-level const/let/function/class declarations persist into later cells; reset, cancelling a running cell, its",
  "timeout, a reload or a host restart discard them (the result says so). Globals: tools.<name>(args) returns a",
  "Promise of the tool's result: Corral tools resolve to their canonical receipt object (check .state; refusals",
  "resolve too), other tools to their text; a refused or failed call without a structured result rejects with an",
  "Error whose name, message and code survive JSON.stringify. print/console.* and display(value or",
  "{type:\"image\",data,mimeType}) emit output; setTimeout/clearTimeout. There is no filesystem, network, process",
  "or import of its own: every capability is a tool call with the same authority, confirmations and receipts as a",
  "direct call. Only printed/displayed output and an explicit `return` value reach you, so keep large intermediate",
  "results in variables. Every nested failure is summarized anyway, with its child identity and recorded effect.",
  "Use Promise.all/allSettled for independent calls and await for dependent ones. A cell still running after",
  `wait_ms (default ${DEFAULT_WAIT_MS}) detaches: you get its handle now and exactly one message when it settles;`,
  "keep working, do not poll. {cancel:\"cell-N\"} stops a cell (its variables are lost); {reset:true} discards all",
  "variables first. A failed cell is never re-run: committed nested effects stand, so reconcile an uncertain one",
  "with corral_receipt_lookup instead of re-running the cell.",
].join(" ");

const Parameters = Type.Object({
  code: Type.Optional(Type.String({ minLength: 1, maxLength: 256 * 1024, description: "Raw JavaScript, not a JSON string or fence" })),
  wait_ms: Type.Optional(Type.Integer({ minimum: 0, maximum: MAX_WAIT_MS, description: "Foreground window before the cell detaches" })),
  cancel: Type.Optional(Type.String({ pattern: "^cell-[0-9]{1,12}$", description: "Stop this live cell instead of running code" })),
  reset: Type.Optional(Type.Boolean({ description: "Discard every kernel variable, then run code if given" })),
});

type Params = { code?: string; wait_ms?: number; cancel?: string; reset?: boolean };

function answer(reports: readonly CellReport[], fallback: string) {
  const content = reports.flatMap(reportContent);
  const failed = reports.some((report) => !["ok", "detached", "reset", "not_found", "cancelled"].includes(report.status));
  return {
    content: content.length > 0 ? content : [{ type: "text" as const, text: fallback }],
    details: { cells: reports },
    isError: failed,
    ...(reports.some((report) => report.terminate) ? { terminate: true } : {}),
  };
}

/** `callable` filters Pi's callable tools through the host's execution check for nested calls. */
export function codemodeTool(runtime: CodeRuntime, callable: (names: readonly string[]) => string[]): ToolDefinition {
  return {
    name: CODEMODE_TOOL, label: "Code Mode", exposure: "model-only", description: DESCRIPTION, parameters: Parameters,
    promptSnippet: "Run JavaScript cells that call other tools on a persistent kernel",
    prepareLoadout: (loadout) => {
      const names = callable(loadout.callable.map((tool) => tool.name).filter((name) => name !== CODEMODE_TOOL));
      // The legacy launch alias stays callable, but new scripts should use bash({ wait_ms: 0 }).
      const advertised = names.includes("bash") ? names.filter(name => name !== "job_start") : names;
      return { descriptions: { [CODEMODE_TOOL]: `${DESCRIPTION}\nCallable as tools.<name>: ${advertised.join(", ") || "none"}.` } };
    },
    async execute(toolCallId, params: Params, signal, _update, ctx) {
      if (params.cancel !== undefined) {
        if (params.code !== undefined || params.reset) {
          return { content: [{ type: "text", text: "cancel takes no code or reset" }], details: { cells: [] }, isError: true };
        }
        return answer([await runtime.cancel(params.cancel)], "");
      }
      if (params.code === undefined && !params.reset) {
        return { content: [{ type: "text", text: "codemode needs code, cancel or reset" }], details: { cells: [] }, isError: true };
      }
      const lost = params.reset ? await runtime.reset() : [];
      if (params.code === undefined) return answer(lost, "Kernel reset: every variable was discarded.");
      return answer([...lost, await runtime.run(toolCallId, params.code, ctx, signal, params.wait_ms ?? DEFAULT_WAIT_MS)], "");
    },
  };
}
