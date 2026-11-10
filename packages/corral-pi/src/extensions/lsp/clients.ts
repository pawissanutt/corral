// Per-key LSP recovery is one operation; shutdown owns every startup, not a cache snapshot.
import { LspClient, LspError } from "./client.ts";

export class LspClients {
  private readonly clients = new Map<string, Promise<LspClient>>();
  private readonly flights = new Map<string, Promise<LspClient>>();
  private readonly owned = new Set<Promise<LspClient>>();
  private stopped = false;
  private closing: Promise<void> | undefined;

  async get(key: string, start: () => Promise<LspClient>): Promise<LspClient> {
    if (this.stopped) throw new LspError("lsp_server_unavailable", "LSP manager stopped; no executor started");
    const pending = this.flights.get(key);
    if (pending) return pending;
    const existing = this.clients.get(key);
    // Publish the entire recovery/startup operation before any callback or await. Calls
    // sharing a key share cleanup, replacement and its failure, not just initial start.
    const flight = Promise.resolve().then(async () => {
      if (existing) {
        const current = await existing.catch(error => {
          if (!LspClient.startupSettled(error)) throw error;
          return undefined;
        });
        if (current?.alive && !this.stopped) return current;
        if (current) await current.shutdown();
      }
      if (this.stopped) throw new LspError("lsp_server_unavailable", "LSP manager stopped during startup");
      const started = start();
      this.owned.add(started);
      this.clients.set(key, started);
      return started;
    });
    this.flights.set(key, flight);
    try { return await flight; }
    finally { if (this.flights.get(key) === flight) this.flights.delete(key); }
  }

  shutdown(): Promise<void> {
    this.stopped = true;
    return this.closing ??= (async () => {
      await Promise.allSettled([...this.flights.values()]);
      // Cache entries can be replaced; cleanup ownership cannot. Known-settled startup
      // failures are already stopped, while every other failure remains a refusal.
      const results = await Promise.allSettled([...this.owned].map(async started => {
        let server: LspClient;
        try { server = await started; }
        catch (error) { if (LspClient.startupSettled(error)) return; throw error; }
        await server.shutdown();
      }));
      const failures = results.flatMap(result => result.status === "rejected" ? [result.reason] : []);
      if (failures.length) throw new AggregateError(failures, "LSP shutdown/identity unknown; ownership retained");
    })();
  }
}
