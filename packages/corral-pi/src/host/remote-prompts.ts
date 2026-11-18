import type { EventLog } from "../bridge/events.ts";
import type { PendingQuestion } from "./pending-questions.ts";

/** Project the durable question state onto the session socket. */
export class RemotePrompts {
  constructor(private readonly events: EventLog) {}

  changed(question: PendingQuestion): void {
    this.events.emit("owner_prompt", { question: question.id, status: question.status, prompts: question.questions });
  }
}
