// Retry observation/delivery only. Commands and hooks are never replayed by this path.
export async function deliverNotice(
  pending: () => boolean,
  send: () => Promise<string>,
): Promise<boolean> {
  for (let attempt = 0; attempt < 4 && pending(); attempt++) {
    if (attempt) await new Promise<void>(resolve => { const timer = setTimeout(resolve, 250 * 2 ** (attempt - 1)); timer.unref(); });
    if (!pending()) return false;
    try {
      if (["started", "queued", "handled", "duplicate"].includes(await send())) return true;
    } catch { /* Keep the same delivery identity across uncertain acknowledgements. */ }
  }
  return false;
}
