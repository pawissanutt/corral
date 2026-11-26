// An assistant takeover is local to this host, never reconstructed from the transcript.
// The host owns the OS lock itself: no helper's death can release it while the host is writing.
import { randomUUID } from "node:crypto";
import { basename, dirname, isAbsolute } from "node:path";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { z } from "zod";
import { resolveToolPath, within } from "../roles/assistant/paths.ts";
import { classifyWriteScope, type WriteScopeDecision } from "../roles/assistant/write-scope.ts";
import { APPROVAL_CHOICES as CHOICE, type PendingQuestion, type PendingQuestions } from "./pending-questions.ts";
import { WorktreeFiles, tryWorktreeLock } from "./worktree-files.ts";

export const WORKTREE_ACCESS_COMMAND = "worktree-access";
export const WORKTREE_REQUEST_TOOL = "request_task_worktree";
export const WORKTREE_ACCESS_ENTRY = "corral_worktree_access";
export const TASK_ID = /^T-[0-9]+$/;

const absolute = z.string().refine(isAbsolute);
const decimal = z.string().regex(/^\d+$/);
const Snapshot = z.object({ task: z.string(), run: z.string(), checkout: absolute, lock: absolute,
  device: decimal, inode: decimal, lockDevice: decimal, lockInode: decimal });
type Snapshot = z.infer<typeof Snapshot>;
type Lock = ReturnType<typeof tryWorktreeLock>;
type Attempt = { snapshot: Snapshot; session: string; files: WorktreeFiles; question?: string;
  acquiring?: Promise<void>; lock?: Lock; granted?: boolean };
type AccessState = "pending" | "granted" | "denied" | "revoked" | "unavailable" | "cancelled";

interface Host {
  readonly cwd: string;
  readonly questions: PendingQuestions;
  session(): string;
  ready(): boolean;
  command(argv: readonly string[]): Promise<unknown>;
  audit(record: Record<string, unknown>): void;
}

const describe = (error: unknown) => error instanceof Error ? error.message : String(error);
function quote(text: string): string {
  return JSON.stringify(text).replace(/[\x7f-\x9f\p{Cf}\u2028\u2029]/gu,
    char => char.split("").map(unit => `\\u${unit.charCodeAt(0).toString(16).padStart(4, "0")}`).join(""));
}

export class WorktreeAccess {
  private attempt?: Attempt;
  private inquiry?: symbol;
  private readonly denials = new Set<string>();
  private last?: string;

  constructor(private readonly host: Host) {}

  tool(): ToolDefinition {
    return {
      name: WORKTREE_REQUEST_TOOL, label: "Request task worktree",
      description: "Ask the owner for this session's exclusive edit/write access to one stopped task's registered worktree. "
        + "Does not stop the worker, allow shell access, or submit, approve, or land changes.",
      parameters: Type.Object({ task: Type.String({ pattern: TASK_ID.source, maxLength: 32 }) }),
      execute: async (_id, { task }, signal) => ({
        content: [{ type: "text", text: await this.request(task, signal) }], details: undefined,
      }),
    };
  }

  status(): string {
    const attempt = this.attempt;
    if (!attempt) return this.last ?? "No task worktree access in this session.";
    const { snapshot, session, question, granted } = attempt;
    const phase = granted ? "Granted" : attempt.acquiring ? "Checking owner approval" : `Waiting for owner approval ${question ?? ""}`;
    return `${phase}: ${snapshot.task} / ${snapshot.run}\nCheckout: ${quote(snapshot.checkout)}\nSession: ${quote(session)}\n`
      + "Scope: edit and write only; until revoked, handed back or this host session ends.";
  }

  async request(task: string, signal?: AbortSignal): Promise<string> {
    if (!TASK_ID.test(task) || task.length > 32) throw new Error("Expected a registered task ID (T-123).");
    if (!this.host.ready() || signal?.aborted) throw new Error("Task worktree requests are unavailable in this session phase.");
    if (this.denials.has(task)) return `The owner denied task worktree access for ${task} in this session.`;
    if (this.inquiry || this.attempt) {
      if (this.attempt?.snapshot.task === task) { await this.attempt.acquiring; return this.status(); }
      return "Only one task worktree request or grant is allowed at a time. The owner can hand it back with /worktree-access revoke.";
    }
    const inquiry = this.inquiry = Symbol();
    const session = this.host.session();
    let attempt: Attempt | undefined;
    try {
      const snapshot = Snapshot.parse(await this.host.command(["takeover-probe", task]));
      if (this.inquiry !== inquiry || !this.host.ready() || this.host.session() !== session || signal?.aborted) {
        return "Task worktree request cancelled; no access granted.";
      }
      if (snapshot.task !== task || basename(snapshot.checkout) !== task || basename(dirname(snapshot.checkout)) !== ".worktrees") {
        throw new Error("Engine did not return the requested task's dedicated checkout.");
      }
      const prompt = `Allow this assistant to edit a task worktree?\nTask: ${task} / ${snapshot.run}\n`
        + `Canonical checkout: ${quote(snapshot.checkout)}\nRequesting session: ${quote(session)}\n`
        + "Scope: source files via edit and write only. No Git or Corral state, shell permission, or board handoff.\n"
        + "Duration: this host session, until the owner revokes or hands back access.";
      if (Buffer.byteLength(prompt) > 1000 || Buffer.byteLength(JSON.stringify(prompt)) > 1002) {
        throw new Error("The full checkout and session do not fit in the owner approval card.");
      }
      // Hold the directory open during the prompt too; an unlink/recreate cannot reuse its inode.
      const files = new WorktreeFiles(snapshot.checkout, snapshot.device, snapshot.inode);
      this.attempt = attempt = { snapshot, session, files };
      attempt.question = this.host.questions.ask(randomUUID(), [{ question: prompt,
        options: [CHOICE.session, CHOICE.deny], freeText: false }], "task_worktree").id;
      this.record(task, session, "pending", undefined, attempt);
      return `${this.status()}\nUse /questions or the app; ordinary chat does not grant access. Use absolute paths after approval.`;
    } catch (error) {
      if (attempt && this.attempt === attempt) this.detach(attempt);
      if (this.inquiry !== inquiry) return "Task worktree request cancelled; no access granted.";
      const reason = describe(error);
      this.record(task, session, "unavailable", reason, attempt);
      return this.status();
    } finally {
      if (this.inquiry === inquiry) this.inquiry = undefined;
    }
  }

  observe(question: PendingQuestion): void {
    const attempt = this.attempt;
    if (!attempt || attempt.question !== question.id || attempt.acquiring || attempt.granted || question.status === "pending") return;
    if (question.status === "answered" && question.answers?.[0]?.option === CHOICE.session) {
      attempt.acquiring = this.activate(attempt);
      void attempt.acquiring.catch(() => {}); // a failing journal must not create a grant or an unhandled rejection
    } else {
      const denied = question.status === "answered" && question.answers?.[0]?.option === CHOICE.deny;
      if (denied) this.denials.add(attempt.snapshot.task);
      this.detach(attempt);
      this.record(attempt.snapshot.task, attempt.session, denied ? "denied" : "cancelled", denied ? "owner denied" : question.status, attempt);
    }
  }

  private async activate(attempt: Attempt): Promise<void> {
    try {
      this.assertCurrent(attempt);
      const { snapshot } = attempt;
      attempt.lock = tryWorktreeLock(snapshot.lock, true);
      if (attempt.lock.device !== snapshot.lockDevice || attempt.lock.inode !== snapshot.lockInode) {
        throw new Error("The worktree lock file changed; request fresh owner approval.");
      }
      await this.verify(attempt);
      // No grant if journaling the owner's choice and resulting effect fails.
      this.record(snapshot.task, attempt.session, "granted", undefined, attempt);
      attempt.granted = true;
    } catch (error) {
      if (this.attempt === attempt) {
        this.detach(attempt);
        this.record(attempt.snapshot.task, attempt.session, "unavailable", describe(error), attempt);
      }
    }
  }

  /** Called only by trusted host lifecycle/owner commands, never by a model tool. */
  revoke(reason: string): string {
    this.inquiry = undefined;
    const attempt = this.attempt;
    if (!attempt) return this.status();
    this.detach(attempt);
    this.record(attempt.snapshot.task, attempt.session, "revoked", reason, attempt);
    return this.status();
  }

  private detach(attempt: Attempt): void {
    this.attempt = undefined;
    // Filesystem effects are synchronous and check the live attempt immediately before use.
    attempt.lock?.close();
    attempt.files.close();
    if (attempt.question && this.host.questions.get(attempt.question)?.status === "pending") this.host.questions.cancel(attempt.question);
  }

  private record(task: string, session: string, state: AccessState, reason?: string, attempt?: Attempt): void {
    const note = reason?.slice(0, 400);
    const location = attempt ? ` / ${attempt.snapshot.run} at ${quote(attempt.snapshot.checkout)}` : "";
    this.last = `Task worktree access ${state}: ${task}${location}${note ? ` (${quote(note)})` : ""}.`;
    this.host.audit({ task, session, state, at: new Date().toISOString(), ...(note ? { reason: note } : {}),
      ...(attempt ? { run: attempt.snapshot.run, checkout: attempt.snapshot.checkout, question: attempt.question } : {}) });
  }

  private assertCurrent(attempt: Attempt, signal?: AbortSignal): void {
    if (signal?.aborted || this.attempt !== attempt || !this.host.ready() || this.host.session() !== attempt.session) {
      throw new Error("Task worktree access is no longer current; nothing further was written.");
    }
    attempt.lock?.check();
    attempt.files.check();
  }

  private async verify(attempt: Attempt, signal?: AbortSignal): Promise<void> {
    this.assertCurrent(attempt, signal);
    const expected = attempt.snapshot;
    const current = Snapshot.parse(await this.host.command(["takeover-verify", expected.task]));
    this.assertCurrent(attempt, signal);
    if (Object.keys(expected).some(key => expected[key as keyof Snapshot] !== current[key as keyof Snapshot])) {
      throw new Error("The registered task run or worktree changed; request fresh owner approval.");
    }
  }

  private selected(path: string): Attempt | undefined {
    const attempt = this.attempt;
    if (!attempt?.granted && !attempt?.acquiring) return;
    try {
      if (within(resolveToolPath(path, this.host.cwd), attempt.snapshot.checkout)) return attempt;
    } catch { /* malformed paths fall through to the existing refusal */ }
  }

  // Only a lexical path in the selected checkout is considered for this exception. Canonical
  // escapes and all hard denies still fail. The SDK wrapper separately rechecks at execution.
  decision(path: string): WriteScopeDecision | undefined {
    const attempt = this.selected(path);
    if (!attempt) return;
    const decision = classifyWriteScope(path, { cwd: this.host.cwd, root: attempt.snapshot.checkout, buildWorktree: true });
    return decision.action === "ask" ? { action: "deny", reason: "Task worktree access does not allow paths outside the approved checkout." } : decision;
  }

  async use<T>(path: string, signal: AbortSignal | undefined,
    operation: (files: () => WorktreeFiles) => Promise<T>, ordinary: () => Promise<T>): Promise<T> {
    const attempt = this.selected(path);
    if (!attempt) return ordinary();
    await attempt.acquiring;
    if (!attempt.granted) throw new Error(this.status());
    const decision = this.decision(path);
    if (decision?.action !== "allow") throw new Error(decision?.action === "deny" ? decision.reason : "Task worktree access is no longer current.");
    try { await this.verify(attempt, signal); }
    catch (error) {
      if (this.attempt === attempt && !signal?.aborted) this.revoke(describe(error));
      throw error;
    }
    return operation(() => {
      this.assertCurrent(attempt, signal);
      return attempt.files;
    });
  }
}
