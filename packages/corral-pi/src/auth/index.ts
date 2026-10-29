// The production auth module (F6, owner host-install): Pi reads and refreshes the bound
// provider's subscription OAuth credential in the corral-owned auth directory that
// `corral agent login` provisioned. It supplies no transport, so the host's guarded fetch sends
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
    if (PROVIDERS[context.provider].auth !== "oauth") return NO_STORED_CREDENTIAL;
    store = new FileCredentialStore(authDir(), context.provider);
    if (!store.present()) {
      const name = PROVIDERS[context.provider].label.split(" (")[0];
      throw new CredentialsUnavailable(`no ${name} login (provider ${context.provider}) in ${store.dir}; ` +
        `run \`corral agent login --provider ${context.provider}\``);
    }
    return store;
  },
  state() {
    return { module: "corral-pi", credential: store?.present() ? "present" : "absent" };
  },
};

export default auth;
