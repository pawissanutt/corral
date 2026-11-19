// Pi reads the selected provider's credential from the corral-owned auth directory.
// Production supplies no transport, so the host's guarded fetch sends
// over the real network. Amazon Bedrock has no OAuth credential: its host signs with the bound AWS
// profile (src/auth/aws.ts), so Pi gets an empty store that refuses writes.
import type { CredentialStore } from "@earendil-works/pi-ai";

import type { AuthModule } from "./types.ts";
import { CredentialsUnavailable, authDir } from "./paths.ts";
import { HOST_PROVIDER_IDS, PROVIDERS } from "./providers.ts";
import { FileCredentialStore } from "./store.ts";

let store: FileCredentialStore | undefined;

const NO_STORED_CREDENTIAL: CredentialStore = {
  read: async () => undefined,
  list: async () => [],
  modify: async () => {
    throw new Error("this provider never stores a credential");
  },
  delete: async () => {
    throw new Error("this provider never stores a credential");
  },
};

const auth: AuthModule = {
  provider: "openai-codex",
  providers: HOST_PROVIDER_IDS,
  credentials(context) {
    const kind = PROVIDERS[context.provider].auth;
    if (kind !== "api_key" && kind !== "oauth") return NO_STORED_CREDENTIAL;
    const candidate = new FileCredentialStore(authDir(), context.provider);
    if (!candidate.present()) {
      const name = PROVIDERS[context.provider].label.split(" (")[0];
      throw new CredentialsUnavailable(kind === "api_key"
        ? `no platform API key for ${context.provider}; run \`corral-pi login --provider ${context.provider}\``
        : `no ${name} login (provider ${context.provider}) in ${candidate.dir}; run \`corral-pi login --provider ${context.provider}\``);
    }
    store = candidate;
    return candidate;
  },
  state() {
    return { module: "corral-pi", credential: store?.present() ? "present" : "absent" };
  },
};

export default auth;
