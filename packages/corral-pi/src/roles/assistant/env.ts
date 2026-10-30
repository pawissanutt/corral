// The session the assistant's shell commands run for. corral's CLI entry reads these variables
// to attribute and classify each command; they never carry a secret.
import type { HostIdentity } from "../types.ts";

export const SESSION_ENV = "CORRAL_ASSISTANT";
export const GENERATION_ENV = "CORRAL_ASSISTANT_GENERATION";

/** Exports an assistant host's session into the environment its tools' subprocesses inherit. */
export function exportAssistantSession(env: NodeJS.ProcessEnv, identity: HostIdentity): void {
  delete env[SESSION_ENV];
  delete env[GENERATION_ENV];
  if (identity.role !== "assistant" || identity.assistant === undefined) return;
  env[SESSION_ENV] = identity.assistant;
  env[GENERATION_ENV] = String(identity.generation);
}
