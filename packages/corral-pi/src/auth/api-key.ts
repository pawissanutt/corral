// Explicit offline setup only. Never discover keys from env, other harnesses or commands.
import { type ApiProviderId, validApiKey } from "./providers.ts";
import { CredentialsUnavailable } from "./paths.ts";
import { FileCredentialStore, readPrivateFile } from "./store.ts";
import { type KeyTerminal, readHiddenKey } from "./hidden-key.ts";

export async function installApiKey(dir: string, provider: ApiProviderId, inputFile: string): Promise<void> {
  await storeApiKey(dir, provider, readPrivateFile(inputFile, 4096));
}

async function storeApiKey(dir: string, provider: ApiProviderId, input: string): Promise<void> {
  const key = input.trim();
  if (!validApiKey(key)) throw new CredentialsUnavailable("input is not a literal platform API key");
  const store = new FileCredentialStore(dir, provider);
  await store.modify(provider, async () => ({ type: "api_key", key }));
}

/** Reuses the installed login entry, but makes no OAuth, refresh or model requests. */
export async function apiKeyCommand(
  dir: string, provider: ApiProviderId, argv: readonly string[], terminal?: KeyTerminal,
): Promise<void> {
  const actions = ["--logout", "--status", "--key-file", "--prompt"].filter(flag => argv.includes(flag));
  if (actions.length > 1 || actions.some(flag => argv.filter(arg => arg === flag).length !== 1)) {
    throw new CredentialsUnavailable("select exactly one of --prompt, --key-file, --status or --logout");
  }
  const store = new FileCredentialStore(dir, provider);
  const action = actions[0] ?? "--prompt";
  if (action === "--logout") {
    await store.delete(provider);
    process.stdout.write(`${provider}: removed (other providers unchanged)\n`);
  } else if (action === "--status") {
    process.stdout.write(`${provider}: ${store.present() ? "present" : "absent"}\n`);
  } else {
    if (action === "--prompt") {
      await storeApiKey(dir, provider, await readHiddenKey(terminal));
    } else {
      const index = argv.indexOf("--key-file");
      const file = index < 0 ? undefined : argv[index + 1];
      if (!file) throw new CredentialsUnavailable("API setup requires --key-file <protected-0600-file>; never put a key in argv");
      await installApiKey(dir, provider, file);
    }
    process.stdout.write(`${provider}: stored\n`);
  }
}
