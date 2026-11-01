// Only the pinned upstream QuickJS extension: no ambient Pi extension discovery, model
// catalog, classifier or image execution. Every nested call uses ctx.executeTool, so the
// host's common tool_call check and root-call attribution apply, including after reload.
import type { createCodemodeExtension, ExtensionFactory } from "@earendil-works/pi-coding-agent";

export function codemode(factory: typeof createCodemodeExtension): ExtensionFactory {
  return async (pi) => {
    await factory({ mode: "on", models: false })(pi);
    pi.on("before_agent_start", (event) => ({
      systemPrompt: event.systemPrompt + "\nCode Mode: only explicitly allowed receipt-free Corral board reads "
        + "are callable in scripts; call other Corral operations directly at top level. "
        + "A denied/failed nested call rejects with Error.message. When printing Promise.allSettled results, "
        + "map rejected reasons to String(reason?.message ?? reason); JSON.stringify(Error) otherwise hides details as {}.",
    }));
  };
}
