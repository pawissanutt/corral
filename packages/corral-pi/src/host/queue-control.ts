export interface QueuedItem {
  readonly key: string;
  readonly text: string;
  readonly policy: "steer" | "follow_up";
  readonly editable: boolean;
  /** Who queued it: the owner's pane, a corral inbox delivery or an extension; presentation only. */
  readonly origin?: "pane" | "corral-inbox" | "extension";
}

export interface QueueControl {
  /** False when the session's capability set does not allow changing the queue; listing still works. */
  enabled(): boolean;
  edit(key: string): Promise<string | undefined>;
  remove(key: string): Promise<boolean>;
  move(key: string, delta: -1 | 1): Promise<boolean>;
}
