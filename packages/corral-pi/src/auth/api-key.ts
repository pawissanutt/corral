// Explicit offline setup only. Never discover keys from env, other harnesses or commands.
import { CONNECTIONS, type ApiConnectionId, isApiConnection, isConnectionId, validApiKey } from "./connections.ts";
import { CredentialsUnavailable } from "./paths.ts";
import { FileCredentialStore, readPrivateFile } from "./store.ts";

export async function installApiKey(dir: string, connection: ApiConnectionId, inputFile: string): Promise<void> {
  const key = readPrivateFile(inputFile, 4096).trim();
  if (!validApiKey(key)) throw new CredentialsUnavailable("input is not a literal platform API key");
  const store = new FileCredentialStore(dir, CONNECTIONS[connection].provider, {}, connection);
  await store.modify(CONNECTIONS[connection].provider, async () => ({ type: "api_key", key }));
}

/** Reuses the installed login entry, but makes no OAuth, refresh or model requests. */
export async function apiKeyCommand(dir: string, selected: string, argv: readonly string[]): Promise<void> {
  if (!isConnectionId(selected) || !isApiConnection(selected)) throw new CredentialsUnavailable("unknown API connection");
  const store = new FileCredentialStore(dir, CONNECTIONS[selected].provider, {}, selected);
  if (argv.includes("--logout")) {
    await store.delete(CONNECTIONS[selected].provider);
    process.stdout.write(`${selected}: removed (other connections unchanged)\n`);
  } else if (argv.includes("--status")) {
    process.stdout.write(`${selected}: ${store.present() ? "present" : "absent"}; spending disabled\n`);
  } else {
    const index = argv.indexOf("--key-file");
    const file = index < 0 ? undefined : argv[index + 1];
    if (!file) throw new CredentialsUnavailable("API setup requires --key-file <protected-0600-file>; never put a key in argv");
    await installApiKey(dir, selected, file);
    process.stdout.write(`${selected}: stored; key presence does not authorize paid requests\n`);
  }
}
