// Which Pi session a host generation opens (T-252 recovery). Generation 1 starts a fresh session
// in its container's session directory. A resumed generation continues the most recent session
// there, the one the lost generation wrote, so the conversation survives and tool-call request
// IDs (derived from the session ID) stay stable across generations. Pi gives the calls the lost
// host left open synthetic results only in what it sends the model; the file is never
// rewritten, and corral's recovery_notice says what became of each of those calls.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { SessionManager } from "@earendil-works/pi-coding-agent";

export type SessionSdk = Pick<typeof Sdk, "SessionManager">;

export function openSession(sdk: SessionSdk, cwd: string, sessionDir: string, generation: number): SessionManager {
  return generation > 1
    ? sdk.SessionManager.continueRecent(cwd, sessionDir)
    : sdk.SessionManager.create(cwd, sessionDir);
}
