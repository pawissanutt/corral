import { randomUUID } from "node:crypto";
import type { WriteScopeDecision } from "../roles/assistant/write-scope.ts";
import { sha256Hex } from "../tools/corral.ts";
import type { PendingQuestions } from "./pending-questions.ts";

export type WriteTool = "edit" | "write";
export type ShellTool = "bash" | "job_start";

type Input = Record<string, unknown>;

function quoted(text: string): string {
  // Show literal whitespace and bidi/terminal controls instead of letting input change the prompt.
  return JSON.stringify(text).replace(/[\x7f-\x9f\p{Cf}\u2028\u2029]/gu,
    char => char.split("").map(unit => `\\u${unit.charCodeAt(0).toString(16).padStart(4, "0")}`).join(""));
}

// The app caps card question text by both UTF-8 and escaped JSON byte length.
function fitsPrompt(text: string): boolean {
  return Buffer.byteLength(text) <= 1000 && Buffer.byteLength(JSON.stringify(text)) <= 1002;
}

function writeSummary(tool: WriteTool, input: Input): string {
  if (tool === "write" && typeof input.content === "string") {
    return `Content (${Buffer.byteLength(input.content)} bytes): ${quoted(input.content.slice(0, 160))}`
      + (input.content.length > 160 ? " (preview)" : "");
  }
  if (tool === "edit" && Array.isArray(input.edits)) {
    const first = input.edits[0] as { oldText?: string; newText?: string } | undefined;
    return `${input.edits.length} replacement(s). First replacement preview: `
      + `${quoted(first?.oldText?.slice(0, 64) ?? "")} → ${quoted(first?.newText?.slice(0, 64) ?? "")}`;
  }
  return "";
}

/** Live, one-shot bindings to the owner's structured question answers. No binding is restored. */
export class ToolApprovals {
  private readonly requests = new Map<string, { id: string; destination?: string }>();

  constructor(private readonly questions: PendingQuestions, private readonly cwd: string) {}

  clear(): void { this.requests.clear(); }

  private key(tool: string, input: Input): string {
    // Ignore property order, but not whitespace, array order, or any argument's value.
    return sha256Hex(JSON.stringify([this.cwd, tool, input], (_key, value: unknown) =>
      value !== null && typeof value === "object" && !Array.isArray(value)
        ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) : value));
  }

  shell(tool: ShellTool, input: Input): string | undefined {
    const runtime = typeof input.timeout === "number" ? `\nRequested timeout: ${input.timeout} s` : "";
    const wait = tool === "job_start" || input.wait_ms === 0 ? "\nStart in background without waiting."
      : typeof input.wait_ms === "number" ? `\nWait up to ${input.wait_ms} ms for output.` : "";
    return this.confirm(this.key(tool, input), "bash", `Allow this ${tool} command once?\n`
      + `Working directory: ${quoted(this.cwd)}${runtime}${wait}\n${quoted(String(input.command))}`);
  }

  write(tool: WriteTool, input: Input, decision: WriteScopeDecision): string | undefined {
    const key = this.key(tool, input);
    const previous = this.requests.get(key);
    if (previous && (decision.action !== "ask" || previous.destination !== decision.canonical)) {
      this.requests.delete(key);
      this.questions.cancel(previous.id);
      return decision.action === "deny" ? decision.reason
        : "The write destination changed since approval was requested. Review the path before asking again; nothing was written.";
    }
    if (decision.action === "deny") return decision.reason;
    if (decision.action === "allow") return undefined;
    const target = quoted(decision.canonical);
    const via = decision.path === decision.canonical ? "" : `\nRequested as: ${quoted(decision.path)}`;
    const title = `Allow ${tool} outside the session directory once?\nDestination: ${target}${via}`;
    const summary = writeSummary(tool, input);
    const preview = `${title}\n${summary}`;
    return this.confirm(key, "file_write", summary && fitsPrompt(preview) ? preview : title, decision.canonical);
  }

  private confirm(key: string, kind: "bash" | "file_write", question: string, destination?: string): string | undefined {
    const effect = kind === "bash" ? "nothing ran" : "nothing was written";
    const waiting = (id: string) => `Waiting for owner approval ${id} (/questions or the app); ${effect}. `
      + "Retry the exact tool call only after the owner selects allow.";
    const prior = this.requests.get(key);
    if (prior) {
      const answer = this.questions.get(prior.id);
      if (answer?.status === "pending") return waiting(prior.id);
      this.requests.delete(key);
      if (this.questions.allowed(prior.id)) {
        return undefined;
      }
      return `No current owner approval for this tool call; ${effect}.`;
    }
    if (!fitsPrompt(question)) return `The full command or destination does not fit in the owner prompt; ${effect}.`;
    const record = this.questions.ask(randomUUID(), [{ question, options: ["deny", "allow"], freeText: false }], kind);
    this.requests.set(key, { id: record.id, ...(destination === undefined ? {} : { destination }) });
    return waiting(record.id);
  }
}
