// The production auth module (F6, owner host-install): Pi reads and refreshes the OpenAI Codex
// OAuth credential in the corral-owned auth directory that `corral agent login` provisioned.
// It supplies no transport, so the host's guarded fetch sends over the real network.
import type { AuthModule } from "./types.ts";
import { CredentialsUnavailable, authDir } from "./paths.ts";
import { FileCredentialStore } from "./store.ts";

let store: FileCredentialStore | undefined;

const auth: AuthModule = {
  provider: "openai-codex",
  credentials() {
    store = new FileCredentialStore(authDir());
    if (!store.present()) {
      throw new CredentialsUnavailable(`no OpenAI Codex login in ${store.dir}; run \`corral agent login\``);
    }
    return store;
  },
  state() {
    return { module: "corral-pi", credential: store?.present() ? "present" : "absent" };
  },
};

export default auth;
