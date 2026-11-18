// Redraw Pi's pane transcript from the session branch after the host rewrote it (a rewind truncates the JSONL).
// Pi's own /tree navigation runs the same sequence; InteractiveMode keeps these members private, so they are reached
// structurally here and nowhere else.
import type { InteractiveMode } from "@earendil-works/pi-coding-agent";

interface Repaintable {
  readonly chatContainer: { clear(): void };
  renderInitialMessages(): void;
  readonly ui: { requestRender(force?: boolean): void };
}

/** Clear the pane transcript, draw the current branch again and request a frame. */
export function repaintTranscript(terminal: InteractiveMode): void {
  const mode = terminal as unknown as Repaintable;
  mode.chatContainer.clear();
  mode.renderInitialMessages();
  mode.ui.requestRender(true);
}
