export interface QueuedItem {
  readonly key: string;
  readonly text: string;
  readonly policy: "steer" | "follow_up";
  readonly editable: boolean;
}

export interface QueueControl {
  edit(key: string): Promise<string | undefined>;
  remove(key: string): Promise<boolean>;
  move(key: string, delta: -1 | 1): Promise<boolean>;
}
