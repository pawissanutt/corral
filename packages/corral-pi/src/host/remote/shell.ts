import type { AgentSession } from "@earendil-works/pi-coding-agent";
import { BridgeRefusal } from "../../bridge/protocol.ts";
import { utf8Prefix } from "../../roles/assistant/owner-action.ts";
import type { RemoteRegistry } from "./registry.ts";

type BashSession = Pick<AgentSession, "extensionRunner" | "sessionManager" | "recordBashResult" | "executeBash">;

/** Mirror Pi's interactive ! path, including extension policy and custom operations. */
export function registerShell(registry: RemoteRegistry, session: () => BashSession,
  runExclusive: <T>(run: () => Promise<T>) => Promise<T>, completed: (text: string) => void, waitMs = 1000): void {
  registry.register({ name: "owner_bash", parse: args => {
    if (typeof args.command !== "string" || !args.command.trim() || typeof args.exclude_from_context !== "boolean") {
      throw new BridgeRefusal("bad_input", "command and exclude_from_context are required");
    }
    return { command: args.command, excludeFromContext: args.exclude_from_context };
  }, run: async ({ command, excludeFromContext }) => {
    const work = runExclusive(async () => {
      const pi = session();
      const event = await pi.extensionRunner.emitUserBash({ type: "user_bash", command, excludeFromContext, cwd: pi.sessionManager.getCwd() });
      let result;
      if (event?.result) {
        result = event.result;
        pi.recordBashResult(command, result, { excludeFromContext });
      } else {
        result = await pi.executeBash(command, undefined, { excludeFromContext, operations: event?.operations });
      }
      const status = result.cancelled ? "cancelled" : `exit ${result.exitCode ?? "unknown"}`;
      return { text: utf8Prefix(`${status}${result.output ? `\n${result.output}` : ""}`, 4096) };
    });
    // The daemon has a bounded RPC deadline. Long commands continue under the host's lock;
    // their recorded bash entry and completion notice are delivered independently of the RPC.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const running = new Promise<undefined>(resolve => { timer = setTimeout(() => resolve(undefined), waitMs); });
    try {
      const result = await Promise.race([work, running]);
      if (result) return result;
      void work.then(result => completed(result.text), error => completed(`Shell failed: ${String(error)}`));
      return { text: "Shell command running; its result will appear in the session." };
    } finally { clearTimeout(timer); }
  } });
}
