import type { Model } from "@earendil-works/pi-ai";

import type { ProviderId } from "../auth/providers.ts";

/**
 * Models the subscription providers serve that the bundled Pi catalog predates. They resolve
 * only for their own provider and model API, so the network guard and lane checks still apply.
 * A model the models.json file or the bundled catalog defines wins over an entry here.
 */
export const SUPPLEMENTAL_MODELS: Readonly<Partial<Record<ProviderId, readonly Model<any>[]>>> = {
  "openai-codex": [],
  anthropic: [],
};
