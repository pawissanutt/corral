// The auth module interface (F6), loaded by fixed path from src/auth/index.ts. It supplies
// the credential store for the pinned provider route. The host's guarded fetch decides where
// requests may go; a module can only swap the transport behind that guard (fixtures use a
// fake provider transport), never widen the allowed destinations.
import type { CredentialStore } from "@earendil-works/pi-ai";

export type Fetch = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export interface AuthContext {
  /** The only provider pi-native runs use until claude-route-probe records GO (OD5). */
  readonly provider: "openai-codex";
  readonly apiEndpoint: string;
  readonly refreshEndpoint: string;
}

export interface AuthModule {
  readonly provider: "openai-codex";
  /** The credential store handed to Pi's ModelRuntime. */
  credentials(context: AuthContext): CredentialStore;
  /** Transport behind the guard; production omits it and the real network is used. */
  transport?(context: AuthContext): Fetch;
  /** Non-secret readback for get_state (never a credential). */
  state?(): Record<string, unknown>;
}
