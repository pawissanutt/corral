import type { RemoteOwner } from "../remote-owner.ts";
import type { HostSession } from "../session.ts";
import { registerFleet } from "./fleet.ts";

/** Install only the parts this session serves, before publishing its first state. */
export function installRemote(host: HostSession, owner: RemoteOwner, sessionDir: string): void {
  if (host.servesChildren) registerFleet(host.remoteRegistry, sessionDir, child => host.childTranscript(child));
  host.liveEvents.bindState(() => host.remoteState(owner));
}
