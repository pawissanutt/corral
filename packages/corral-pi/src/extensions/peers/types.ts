// Cross-session agent messaging as the fleet runtime sees it. Types only: the transport that
// reaches other corral sessions implements this, and the fleet routes a recipient that is not a
// local child, `parent` or `all` through it.

/** Another live corral session an agent may address. */
export interface PeerInfo {
  /** The recipient key: `T-144_R2`, `pm`, `A-91`. */
  readonly key: string;
  /** `worker`, `pm`, `assistant`, `planner`, `reviewer` or `plan-reviewer`. */
  readonly role: string;
  /** The task or session title, when it has one. */
  readonly title?: string;
  readonly state: "live" | "offline";
}

export interface PeerDelivery {
  /** `delivered`: admitted by the recipient's host. `queued`: stored for a recipient that is not reachable now. */
  readonly state: "delivered" | "queued" | "undeliverable";
  /** Why the message was queued or refused. */
  readonly detail?: string;
  /** The stored message id, when one was assigned: `T-144_R2#7`. */
  readonly id?: string;
}

export interface PeerTransport {
  /** This session's own recipient key; undefined for a session other sessions cannot address. */
  self(): string | undefined;
  roster(signal?: AbortSignal): Promise<readonly PeerInfo[]>;
  /** Sends `text` (at most 8 KiB as UTF-8, framing included) as this session. Never interrupts the recipient's turn. */
  send(to: string, text: string, signal?: AbortSignal): Promise<PeerDelivery>;
}
