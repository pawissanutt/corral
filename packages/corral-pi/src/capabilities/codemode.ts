// Only the pinned upstream QuickJS extension: no ambient Pi extension discovery, model
// catalog, classifier or image execution. Every nested call uses ctx.executeTool, so the
// host's common tool_call check and root-call attribution apply, including after reload.
import type { createCodemodeExtension, ExtensionFactory } from "@earendil-works/pi-coding-agent";

export function codemode(factory: typeof createCodemodeExtension): ExtensionFactory {
  return factory({ mode: "on", models: false });
}
