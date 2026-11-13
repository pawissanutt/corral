// The terminal title leads with what the session needs, so an owner in another herdr pane or window sees it:
// needs you, working, or idle. The title is written only when that state changes; a new needs-you episode rings
// the terminal bell once when the owner turned the bell on (F2 Appearance, off by default).
import { sessionTitle, type TitleAttention } from "./chrome.ts";

export interface AttentionOutput {
  title(text: string): void;
  bell(): void;
}

export class AttentionSignal {
  private last: TitleAttention | undefined;

  constructor(private readonly out: AttentionOutput, private readonly base: string, private readonly ringing: () => boolean) {}

  /** The state this frame; the title changes and the bell rings only on a transition. */
  update(state: TitleAttention): void {
    if (state === this.last) return;
    // The first state after startup is not an episode the owner missed.
    const episode = state === "needs-you" && this.last !== undefined;
    this.last = state;
    this.out.title(sessionTitle(this.base, state));
    if (episode && this.ringing()) this.out.bell();
  }
}
