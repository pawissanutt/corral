// The auth module interface (F6), loaded by fixed path from src/auth/index.ts. It supplies
// the credential store for the bound profile's provider route. The host's guarded fetch decides
// where requests may go; a module can only swap the transport behind that guard (fixtures use a
// fake provider transport), never widen the allowed destinations.
import type { CredentialStore } from "@earendil-works/pi-ai";

import type { ProviderId } from "./providers.ts";

export type Fetch = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export interface AuthContext {
  /** The bound profile's subscription OAuth provider (OD5; Anthropic per T-357/T-479). */
  readonly provider: ProviderId;
  readonly apiEndpoint: string;
  readonly refreshEndpoint: string;
}

export interface AuthModule {
  /** The provider of a module written before T-479; such a module serves only it. */
  readonly provider: "openai-codex";
  /** Every provider the module serves; absent means only `provider`. */
  readonly providers?: readonly ProviderId[];
  /** The credential store handed to Pi's ModelRuntime. */
  credentials(context: AuthContext): CredentialStore;
  /** Transport behind the guard; production omits it and the real network is used. */
  transport?(context: AuthContext): Fetch;
  /** Non-secret readback for get_state (never a credential). */
  state?(): Record<string, unknown>;
}
