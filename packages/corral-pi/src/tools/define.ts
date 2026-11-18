import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { JsonValue } from "@earendil-works/pi-ai";
import { Errors } from "typebox/value";
import type { HostIdentity, CorralToolSpec } from "../roles/types.ts";
import { CorralCommandError, type CorralClient } from "./corral.ts";

export interface ToolHost {
  readonly client: CorralClient;
  readonly identity: HostIdentity;
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
  return {
    name: spec.name,
    label: spec.label,
    description: spec.description,
    exposure: "direct",
    parameters: spec.parameters,
    async execute(_id, params) {
      try {
        const errors = Errors(spec.parameters, params);
        if (errors.length) throw new Error(`Invalid arguments: ${errors.map(error => error.instancePath || "/").join(", ")}`);
        const payload = spec.payload(params as Record<string, unknown>);
        const { task } = host.identity;
        let result: unknown;
        switch (spec.op) {
          case "todo": result = await host.todo(payload); break;
          case "checkpoint": result = host.checkpoint(String(payload.note)); break;
          case "done":
            if (!task) throw new Error("No task is attached to this session");
            result = await host.client.command(["done", task, ...option("--summary", String(payload.summary))]);
            break;
          case "verdict":
            if (!task) throw new Error("No task is attached to this session");
            result = await host.client.command(["verdict", task, String(payload.verdict), ...option("--notes", String(payload.notes))]);
            break;
          case "artifact":
            if (!task) throw new Error("No task is attached to this session");
            result = await host.client.artifact(task, String(payload.path),
              typeof payload.kind === "string" ? payload.kind : undefined,
              typeof payload.title === "string" ? payload.title : undefined);
            break;
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
