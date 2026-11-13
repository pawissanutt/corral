// Esc means "back" in views and docks but "stop the turn" in Chat. After an Esc that navigated (a view change,
// leaving an agent's conversation, a dismissed dock or a closed panel overlay), a further Esc within the guard
// window is ignored, so pressing Esc to get out never also stops the running turn. A single Esc in Chat with
// the composer focused is never delayed; any other key ends the guard.

/** A second Esc this soon after a navigating one is the same "get out" gesture, not a request to stop. */
export const ESC_GUARD_MS = 600;

export class EscGuard {
  private until = 0;

  constructor(private readonly now: () => number) {}

  /** True while a navigating Esc was this recent: the press is ignored. */
  get blocking(): boolean { return this.now() < this.until; }

  /** An Esc just navigated. */
  arm(): void { this.until = this.now() + ESC_GUARD_MS; }

  /** Another key arrived: the next Esc is deliberate. */
  clear(): void { this.until = 0; }
}
