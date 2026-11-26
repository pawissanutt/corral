import { BridgeRefusal, parseQueueWithdraw } from "../../bridge/protocol.ts";
import type { RemoteOwner } from "../remote-owner.ts";
import type { HostSession } from "../session.ts";
import { registerCards } from "./cards.ts";
import { registerFleet } from "./fleet.ts";

/** Install only the parts this session serves, before publishing its first state. */
export function installRemote(host: HostSession, owner: RemoteOwner, sessionDir: string): void {
  if (host.servesChildren) registerFleet(host.remoteRegistry, sessionDir, child => host.childTranscript(child));
  registerCards(host.remoteRegistry, {
    dialogs: host.dialogs, ready: () => host.remoteActionsReady, beforeQuestion: () => owner.resume(),
    reviewAttempt: card => {
      const plan = host.planMode;
      const brief = plan?.review === card ? plan.brief() : undefined;
      return brief?.approval_epoch && brief.approval_host
        ? { approval_epoch: brief.approval_epoch, approval_host: brief.approval_host } : undefined;
    },
  });
  host.remoteBuild.register(host.remoteRegistry);
  host.installRemoteParts();
  host.remoteRegistry.register({ name: "queue_move", parse: args => {
    const key = parseQueueWithdraw(args), { delta } = args;
    if (delta !== -1 && delta !== 1) throw new BridgeRefusal("bad_input", "delta must be -1 or 1");
    return { key, delta } as const;
  }, run: ({ key, delta }) => {
    if (!host.remoteActionsReady) throw new BridgeRefusal("host_not_ready", "session is not accepting queue moves");
    const moved = host.queue.moveOwner(key, delta);
    if (moved) host.liveEvents.stateChanged();
    return { moved };
  } });
  host.liveEvents.bindState(() => host.remoteState(owner));
}
