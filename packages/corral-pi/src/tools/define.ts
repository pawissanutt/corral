import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { JsonValue } from "@earendil-works/pi-ai";
import type { TSchema } from "typebox";
import type { HostIdentity, CorralToolSpec } from "../roles/types.ts";
import { CorralCommandError, type CorralClient } from "./corral.ts";
import { argumentErrors, flatObjectSchema, parseStringifiedArguments, type JsonSchema } from "./schema.ts";

export interface ToolHost {
  readonly client: CorralClient;
  readonly identity: HostIdentity;
  drained(): Promise<unknown>;
  todo(payload: Record<string, unknown>): unknown;
  checkpoint(note: string): unknown;
}

export function corralToolNames(specs: readonly CorralToolSpec[]): string[] {
  return specs.map(spec => spec.name);
}

function option(name: string, value: string): string[] {
  return value.startsWith("-") ? [`${name}=${value}`] : [name, value];
}

export function defineCorralTool(spec: CorralToolSpec, host: ToolHost): ToolDefinition {
  // The model sees one root object; the exact schema still decides the call in execute.
  const advertised = flatObjectSchema(spec.parameters as JsonSchema);
  return {
    name: spec.name,
    label: spec.label,
    description: spec.description,
    exposure: "direct",
    parameters: advertised as TSchema,
    prepareArguments: (args) => parseStringifiedArguments(advertised, args) as Record<string, unknown>,
    async execute(_id, params) {
      try {
        const invalid = argumentErrors(spec.parameters, params);
        if (invalid) throw new Error(`Invalid arguments: ${invalid}`);
        const payload = spec.payload(params as Record<string, unknown>);
        const { task } = host.identity;
        let result: unknown;
        switch (spec.op) {
          case "todo": result = await host.todo(payload); break;
          case "checkpoint": result = host.checkpoint(String(payload.note)); break;
          case "done":
          case "verdict":
            if (!task) throw new Error("No task is attached to this session");
            // The engine can close this host or remove its worktree as soon as
            // it accepts the submission. Flush already-reported requests first.
            await host.drained();
            result = await host.client.command(spec.op === "done"
              ? ["done", task, ...option("--summary", String(payload.summary))]
              : ["verdict", task, String(payload.verdict), ...option("--notes", String(payload.notes))]);
            break;
          case "artifact": {
            const owner = typeof payload.to === "string" ? payload.to : task || host.identity.project;
            if (!owner) throw new Error("Specify `to` with a task ID or project name");
            const applied = await host.client.artifact(owner, String(payload.path), String(payload.kind),
              typeof payload.title === "string" ? payload.title : undefined,
              typeof payload.replaces === "string" ? payload.replaces : undefined);
            result = applied;
            // The engine's JSON encodes the reference as the artifact's owner and ordinal.
            if (applied && typeof applied === "object" && "artifact" in applied) {
              const row = applied.artifact;
              if (row && typeof row === "object" && "task" in row && typeof row.task === "string"
                && "id" in row && typeof row.id === "number") {
                result = { ...applied, reference: `${row.task}:${row.id}` };
              }
            }
            break;
          }
        }
        return { content: [{ type: "text", text: JSON.stringify(result) }], details: result,
          structuredContent: result as JsonValue,
          ...((spec.op === "done" || spec.op === "verdict") ? { terminate: true } : {}) };
      } catch (error) {
        const failure = error instanceof CorralCommandError
          ? { reason: error.reason, ...(error.next ? { next: error.next } : {}) }
          : { reason: error instanceof Error ? error.message : String(error) };
        return { content: [{ type: "text", text: error instanceof CorralCommandError && error.diagnostic
          ? error.diagnostic : JSON.stringify(failure) }], details: failure, isError: true,
          structuredContent: failure as JsonValue };
      }
    },
  };
}
