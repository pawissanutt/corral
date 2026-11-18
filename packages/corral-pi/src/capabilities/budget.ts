import { readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Api, Model } from "@earendil-works/pi-ai";

export const SPENDING_CAP_REACHED = "spending cap reached; use corral retry --profile <p> to start a new run";

export interface SpendingOptions {
  readonly maxUsd?: number;
  readonly paidRoute?: string;
}

export interface SpendingState {
  readonly maxUsd: number;
  readonly spentUsd: number;
  readonly paidRoute?: string;
}

interface SavedSpending extends SpendingState {
  readonly continuation?: string;
}

/** One durable counter shared by every API stream in a host, independent of its transcript. */
export class Spending {
  private constructor(private readonly path: string, private saved: SavedSpending) {}

  static open(dir: string, flags: SpendingOptions): Spending | undefined {
    const path = join(dir, "spend.json");
    let saved: SavedSpending | undefined;
    try { saved = JSON.parse(readFileSync(path, "utf8")); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
    if (saved !== undefined) {
      if (!saved || !Number.isFinite(saved.maxUsd) || saved.maxUsd <= 0 || !Number.isFinite(saved.spentUsd) || saved.spentUsd < 0
        || (saved.continuation !== undefined && typeof saved.continuation !== "string")
        || (saved.paidRoute !== undefined && (typeof saved.paidRoute !== "string" || saved.paidRoute !== saved.continuation))) {
        throw new Error(`Invalid spending state: ${path}`);
      }
      if (flags.maxUsd !== undefined && flags.maxUsd !== saved.maxUsd) throw new Error("Cannot change --max-usd of an existing session");
      if (flags.paidRoute !== undefined && flags.paidRoute !== saved.continuation) throw new Error("Cannot change --paid-route of an existing session");
      return new Spending(path, saved);
    }
    if (flags.maxUsd === undefined) {
      if (flags.paidRoute) throw new Error("--paid-route requires --max-usd");
      return undefined;
    }
    if (!Number.isFinite(flags.maxUsd) || flags.maxUsd <= 0) throw new Error("--max-usd must be a positive dollar amount");
    const spending = new Spending(path, { maxUsd: flags.maxUsd, spentUsd: 0,
      ...(flags.paidRoute ? { continuation: flags.paidRoute } : {}) });
    spending.save(spending.saved);
    return spending;
  }

  get continuation(): string | undefined { return this.saved.continuation; }
  get activeRoute(): string | undefined { return this.saved.paidRoute; }
  get snapshot(): SpendingState {
    const { maxUsd, spentUsd, paidRoute } = this.saved;
    return { maxUsd, spentUsd, ...(paidRoute ? { paidRoute } : {}) };
  }

  beforeRequest(model: Model<Api>): void {
    if (this.saved.spentUsd >= this.saved.maxUsd) throw new Error(SPENDING_CAP_REACHED);
    const rates = [model.cost, ...model.cost.tiers ?? []];
    if (!rates.some(rate => rate.input > 0 || rate.output > 0 || rate.cacheRead > 0 || rate.cacheWrite > 0)) {
      throw new Error(`Cannot meter unpriced API model ${model.provider}/${model.id}`);
    }
  }

  charge(usd: number): void {
    // Avoid binary addition leaving an exact dollar bound just below the threshold.
    if (usd > 0) this.save({ ...this.saved, spentUsd: Number((this.saved.spentUsd + usd).toFixed(12)) });
  }

  continueOn(route: string): boolean {
    if (this.saved.paidRoute === route) return false;
    this.save({ ...this.saved, paidRoute: route });
    return true;
  }

  private save(state: SavedSpending): void {
    const temp = `${this.path}.tmp`;
    writeFileSync(temp, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 });
    renameSync(temp, this.path);
    this.saved = state;
  }
}
