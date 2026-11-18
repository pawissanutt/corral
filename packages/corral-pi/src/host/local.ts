import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { createTodoLedger } from "../build/todo-ledger.ts";
import type { HostIdentity } from "../roles/types.ts";

/** Host-local state is ordinary data, not a request log. */
export class LocalControlPlane {
  private readonly ledger;

  constructor(private readonly dir: string, private readonly identity: HostIdentity) {
    this.ledger = createTodoLedger(dir, identity.role === "worker" ? identity.task : undefined);
  }

  todo(payload: Record<string, unknown>) {
    return this.ledger.handle(payload);
  }

  todoSnapshot(): unknown {
    const { progress, plans, history } = this.ledger.snapshot();
    const todo = { todo: progress, todo_requests: { plans, history, turn: null, continued: null } };
    // The pane's existing widget selectors are role-specific; the underlying state is not.
    if (this.identity.role === "worker") return { runs: [{ id: this.identity.run, progress }] };
    if (this.identity.role === "pm") return { container: todo };
    return { container: { state: todo } };
  }

  auxiliaryTranscript(id: string): string {
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(id)) throw new Error("Invalid auxiliary session id");
    const dir = join(this.dir, "auxiliary");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    return join(dir, `${id}.jsonl`);
  }
}
