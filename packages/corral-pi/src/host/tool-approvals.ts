import { randomUUID } from "node:crypto";
import type { WriteScopeDecision } from "../roles/assistant/write-scope.ts";
import { sha256Hex } from "../tools/corral.ts";
import { APPROVAL_CHOICES as CHOICE, isToolApproval, type PendingQuestion, type PendingQuestions } from "./pending-questions.ts";

export const APPROVAL_TTL = 30 * 60 * 1000;

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

/** Live bindings to structured owner answers. Neither pending requests nor grants are restored. */
export class ToolApprovals {
  private readonly requests = new Map<string, { id: string; scope: string; destination?: string }>();
  private readonly grants = new Map<string, number>();

  constructor(private readonly questions: PendingQuestions, private readonly cwd: string,
    private readonly reusable = false, private readonly now = () => performance.now()) {}

  clear(): void {
    this.requests.clear();
    this.grants.clear();
  }

  /** The question store calls this in the live process after committing the owner's answer. */
  observeAnswer(question: PendingQuestion): void {
    if (!this.reusable || !isToolApproval(question) || question.status !== "answered") return;
    const choice = question.answers?.[0]?.option;
    if (choice !== CHOICE.timed && choice !== CHOICE.session) return;
    const request = [...this.requests.values()].find(request => request.id === question.id);
    if (request) this.grants.set(request.scope, choice === CHOICE.timed ? this.now() + APPROVAL_TTL : Infinity);
  }

  private hasGrant(scope: string): boolean {
    const until = this.grants.get(scope);
    if (until === undefined) return false;
    if (until > this.now()) return true;
    this.grants.delete(scope);
    return false;
  }

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
    const key = this.key(tool, input);
    const title = this.reusable ? `Allow this exact ${tool} invocation?` : `Allow this ${tool} command once?`;
    return this.confirm(key, key, "bash", `${title}\n`
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
    const title = `Allow ${tool} outside the session directory?\nDestination: ${target}${via}`
      + (this.reusable ? "\nA longer grant allows future edits and writes to this file only." : "");
    const summary = writeSummary(tool, input);
    const preview = `${title}\n${summary}`;
    return this.confirm(key, `file:${decision.canonical}`, "file_write",
      summary && fitsPrompt(preview) ? preview : title, decision.canonical);
  }

  private confirm(key: string, scope: string, kind: "bash" | "file_write", question: string, destination?: string): string | undefined {
    const effect = kind === "bash" ? "nothing ran" : "nothing was written";
    const waiting = (id: string) => `Waiting for owner approval ${id} (/questions or the app); ${effect}. `
      + "Retry the exact tool call only after an affirmative owner choice.";
    const grant = this.hasGrant(scope);
    const prior = this.requests.get(key);
    if (prior) {
      const answer = this.questions.get(prior.id);
      if (answer?.status === "pending") return waiting(prior.id);
      this.requests.delete(key);
      if (this.questions.allowed(prior.id) || grant) return undefined;
      const choice = answer?.answers?.[0]?.option;
      if (choice !== CHOICE.timed && choice !== CHOICE.session) return `No current owner approval for this tool call; ${effect}.`;
    }
    if (grant) return undefined;
    if (this.reusable && destination !== undefined) {
      // One live decision per file prevents two cards from conflicting over a reusable scope.
      const pending = [...this.requests.values()].find(request => request.scope === scope &&
        this.questions.get(request.id)?.status === "pending");
      if (pending) return `A different change to this file is waiting for owner approval ${pending.id}; ${effect}. `
        + "Allow once applies only to the original change.";
    }
    if (!fitsPrompt(question)) return `The full command or destination does not fit in the owner prompt; ${effect}.`;
    const options = this.reusable ? [CHOICE.deny, CHOICE.once, CHOICE.timed, CHOICE.session] : [CHOICE.deny, CHOICE.legacyOnce];
    const record = this.questions.ask(randomUUID(), [{ question, options, freeText: false }], kind);
    this.requests.set(key, { id: record.id, scope, ...(destination === undefined ? {} : { destination }) });
    return waiting(record.id);
  }
}
