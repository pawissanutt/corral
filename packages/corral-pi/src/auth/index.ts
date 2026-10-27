// The production auth module (F6, owner host-install): Pi reads and refreshes the bound
// provider's subscription OAuth credential in the corral-owned auth directory that
// `corral agent login` provisioned. It supplies no transport, so the host's guarded fetch sends
// over the real network.
import type { AuthModule } from "./types.ts";
import { CredentialsUnavailable, authDir } from "./paths.ts";
import { PROVIDERS, PROVIDER_IDS } from "./providers.ts";
import { FileCredentialStore } from "./store.ts";

let store: FileCredentialStore | undefined;

const auth: AuthModule = {
  provider: "openai-codex",
  providers: PROVIDER_IDS,
  credentials(context) {
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
