// A scratch relink is a generation handoff, never an in-process cwd or Pi-file switch.
import { z } from "zod";
import type { ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";
import type { EventLog } from "../bridge/events.ts";
import type { CorralClient } from "../tools/corral.ts";

/** Supervisor exit codes: 75 opens a new assistant; 76 resumes this one in its new directory. */
export const NEW_ASSISTANT_EXIT = 75;
export const RELINK_ASSISTANT_EXIT = 76;
const ENTRY = "corral_assistant_relink";
const Directory = z.object({ path: z.string().startsWith("/"), branch: z.string().nullable() });
const Snapshot = z.object({
  session: z.object({ id: z.string(), ended: z.null() }),
  directory: Directory,
});
const Pending = z.object({ generation: z.number().int().positive(), directory: Directory });

interface RelinkOptions {
  readonly assistant: string;
  readonly generation: number;
  readonly cwd: string;
  readonly manager: SessionManager;
  readonly client: CorralClient;
  readonly events: EventLog;
  readonly request: () => void;
}

export class AssistantRelink {
  constructor(private readonly options: RelinkOptions) {}

  /** Only a committed turn-end calls this; recheck idle after the asynchronous canonical read. */
  async check(idle: () => boolean): Promise<boolean> {
    const o = this.options;
    let snapshot: z.infer<typeof Snapshot>;
    try {
      snapshot = Snapshot.parse(await o.client.todoSnapshot());
    } catch (error) {
      if (!(error instanceof Error)) throw error;
      o.events.emit("assistant_relink_check_failed", { reason: error.message.slice(0, 256) });
      return false;
    }
    if (snapshot.session.id !== o.assistant || snapshot.directory.path === o.cwd || !idle()) return false;
    // This marker travels in the SAME Pi file. A failed restart remains manually resumable,
    // including its automatic continuation; no owner input is fabricated.
    o.manager.appendCustomEntry(ENTRY, { generation: o.generation, directory: snapshot.directory });
    o.events.emit("assistant_relink_requested", { directory: snapshot.directory });
    o.request();
    return true;
  }

  extend(pi: ExtensionAPI): void {
    pi.on("session_start", (event) => {
      if (event.reason !== "startup") return;
      const o = this.options;
      const last = o.manager.getEntries().findLast(entry => entry.type === "custom" && entry.customType === ENTRY);
      if (!last || last.type !== "custom") return;
      const pending = Pending.safeParse(last.data);
      if (!pending.success || pending.data.generation >= o.generation) return;
      // Consume before submitting: reloads and repeated extension attachment cannot repeat it.
      o.manager.appendCustomEntry(ENTRY, { generation: o.generation });
      const directory = pending.data.directory;
      const branch = directory.path === o.cwd && directory.branch ? ` on branch ${directory.branch}` : "";
      pi.sendUserMessage(`You now work in ${o.cwd}${branch}. Continue the task from this conversation.`);
      o.events.emit("assistant_relink_continued", { cwd: o.cwd, generation: o.generation });
    });
  }
}
