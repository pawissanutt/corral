// InteractiveMode can stop itself after the bridge has already stopped it. Keep the
// shutdown listener until exit: Node may emit a tty error after setRawMode returns.
export function guardTerminalStop(terminal: { stop(): void }, stdin: NodeJS.ReadStream): void {
  const stop = terminal.stop.bind(terminal);
  let stopped = false;
  const terminalGone = (error: unknown): boolean =>
    error instanceof Error && ["EIO", "EBADF", "ENOTTY"].includes((error as NodeJS.ErrnoException).code ?? "");
  terminal.stop = () => {
    if (stopped) return;
    stopped = true;
    stdin.on("error", (error) => {
      if (!terminalGone(error)) throw error;
    });
    try { stop(); } catch (error) { if (!terminalGone(error)) throw error; }
  };
}
