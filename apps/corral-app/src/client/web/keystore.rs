use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct WebCredential {
    pub(super) v: u32,
    pub(super) device: String,
    pub(super) name: String,
    pub(super) private: String,
    pub(super) daemon: String,
    pub(super) port: u16,
    #[serde(default)]
    pub(super) endpoint: Option<String>,
}

pub(super) type Stored = super::identity::Stored<WebCredential>;

#[wasm_bindgen(inline_js = r#"
function storageError(action, error) {
    const detail = error && error.message ? error.message : String(error);
    return new Error(`${action}. ${detail}`);
}

function browserCrypto() {
    if (!globalThis.crypto || !globalThis.crypto.subtle) {
        throw new Error("Browser pairing needs WebCrypto; open Corral on HTTPS or localhost in a browser that supports it.");
    }
    return globalThis.crypto;
}

function openCredentials() {
    return new Promise((resolve, reject) => {
        let abandoned = false;
        const request = indexedDB.open("corral-app", 1);
        request.onupgradeneeded = () => {
            const database = request.result;
            if (!database.objectStoreNames.contains("credentials")) {
                database.createObjectStore("credentials");
            }
        };
        request.onerror = () => reject(request.error || new Error("IndexedDB could not be opened."));
        request.onblocked = () => {
            abandoned = true;
            reject(new Error("Close other Corral tabs, then try again; they are blocking the pairing database."));
        };
        request.onsuccess = () => {
            const database = request.result;
            database.onversionchange = () => database.close();
            if (abandoned) {
                database.close();
            } else {
                resolve(database);
            }
        };
    });
}

function credentialTransaction(database, mode, operation) {
    return new Promise((resolve, reject) => {
        const transaction = database.transaction("credentials", mode);
        let value;
        // A successful request can still be rolled back before the transaction commits.
        transaction.oncomplete = () => resolve(value);
        transaction.onabort = () => reject(transaction.error || new Error("The pairing storage transaction was aborted."));
        transaction.onerror = () => reject(transaction.error || new Error("The pairing storage transaction failed."));
        const request = operation(transaction.objectStore("credentials"));
        request.onsuccess = () => { value = request.result; };
        request.onerror = () => reject(request.error || new Error("The pairing storage request failed."));
    });
}

export async function corral_credential_load() {
    let database;
    let stored;
    try {
        database = await openCredentials();
        const records = await credentialTransaction(database, "readonly", store => store.getAll());
        // Never decrypt or resume an identity after an acknowledged forget intent.
        if (records.some(record => record.forget === true)) return false;
        stored = records[0];
    } catch (error) {
        throw storageError("Could not load this browser's pairing; allow site storage and reload Corral", error);
    } finally {
        if (database) database.close();
    }
    if (stored === undefined) return null;
    try {
        const plaintext = await browserCrypto().subtle.decrypt(
            { name: "AES-GCM", iv: stored.iv }, stored.key, stored.data
        );
        return new Uint8Array(plaintext);
    } catch (error) {
        throw storageError("Could not unlock this browser's stored pairing; forget this browser and pair it again", error);
    }
}

export async function corral_credential_save(bytes) {
    // The wasm argument is a borrowed view, so own it before the first await.
    const plaintext = new Uint8Array(bytes);
    let database;
    try {
        const crypto = browserCrypto();
        const key = await crypto.subtle.generateKey(
            { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]
        );
        const iv = crypto.getRandomValues(new Uint8Array(12));
        const data = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, plaintext);
        database = await openCredentials();
        // Serialize the tombstone check and credential write so another tab cannot
        // resurrect an identity while Forget is removing its local namespaces.
        await new Promise((resolve, reject) => {
            const transaction = database.transaction("credentials", "readwrite");
            let forgetting = false;
            transaction.oncomplete = () => resolve();
            transaction.onabort = () => reject(forgetting
                ? new Error("Forgetting this browser is incomplete; retry Forget before pairing.")
                : transaction.error || new Error("The pairing storage transaction was aborted."));
            const store = transaction.objectStore("credentials");
            const request = store.get("forget");
            request.onsuccess = () => {
                if (request.result !== undefined) {
                    forgetting = true;
                    transaction.abort();
                } else {
                    store.put({ key, iv, data }, "device");
                }
            };
        });
    } catch (error) {
        throw storageError("Could not save this browser's pairing; allow persistent site storage and try pairing again", error);
    } finally {
        plaintext.fill(0);
        if (database) database.close();
    }
}

export async function corral_credential_begin_forget() {
    let database;
    try {
        database = await openCredentials();
        await credentialTransaction(database, "readwrite", store => store.put({ forget: true }, "forget"));
    } catch (error) {
        throw storageError("Could not record forgetting this browser; allow site storage and retry Forget", error);
    } finally {
        if (database) database.close();
    }
}

export async function corral_credential_finish_forget() {
    let database;
    try {
        database = await openCredentials();
        await credentialTransaction(database, "readwrite", store => store.delete("forget"));
    } catch (error) {
        throw storageError("Could not finish forgetting this browser; retry Forget", error);
    } finally {
        if (database) database.close();
    }
}

export async function corral_credential_clear() {
    let database;
    try {
        database = await openCredentials();
        await credentialTransaction(database, "readwrite", store => store.delete("device"));
    } catch (error) {
        throw storageError("Could not forget this browser's pairing; allow site storage and try again", error);
    } finally {
        if (database) database.close();
    }
}
"#)]
extern "C" {
    fn corral_credential_load() -> js_sys::Promise;
    fn corral_credential_save(bytes: &[u8]) -> js_sys::Promise;
    fn corral_credential_begin_forget() -> js_sys::Promise;
    fn corral_credential_finish_forget() -> js_sys::Promise;
    fn corral_credential_clear() -> js_sys::Promise;
}

pub(super) async fn load() -> Result<Stored, String> {
    let value = JsFuture::from(corral_credential_load())
        .await
        .map_err(storage_error)?;
    if value.is_null() {
        return Ok(Stored::Missing);
    }
    if value.as_bool() == Some(false) {
        return Ok(Stored::ForgetPending);
    }
    let bytes = value
        .dyn_into::<js_sys::Uint8Array>()
        .map_err(|_| invalid_credential())?;
    let credential: WebCredential =
        serde_json::from_slice(&bytes.to_vec()).map_err(|_| invalid_credential())?;
    if credential.v != 1
        || !valid_key(&credential.private)
        || !valid_key(&credential.daemon)
        || credential
            .endpoint
            .as_deref()
            .is_some_and(|endpoint| !corral_protocol::device::valid_endpoint(endpoint))
    {
        return Err(invalid_credential());
    }
    Ok(Stored::Credential(credential))
}

pub(super) async fn save(credential: &WebCredential) -> Result<(), String> {
    let bytes = serde_json::to_vec(credential)
        .map_err(|error| format!("Could not encode this browser's pairing: {error}"))?;
    JsFuture::from(corral_credential_save(&bytes))
        .await
        .map_err(storage_error)?;
    Ok(())
}

pub(super) async fn begin_forget() -> Result<(), String> {
    JsFuture::from(corral_credential_begin_forget())
        .await
        .map_err(storage_error)?;
    Ok(())
}

pub(super) async fn finish_forget() -> Result<(), String> {
    JsFuture::from(corral_credential_finish_forget())
        .await
        .map_err(storage_error)?;
    Ok(())
}

pub(super) async fn clear() -> Result<(), String> {
    JsFuture::from(corral_credential_clear())
        .await
        .map_err(storage_error)?;
    Ok(())
}

fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid_credential() -> String {
    "This browser's stored pairing is damaged or unsupported; forget this browser and pair it again."
        .into()
}

fn storage_error(error: JsValue) -> String {
    if let Some(error) = error.dyn_ref::<js_sys::Error>() {
        error.message().into()
    } else if let Some(message) = error.as_string() {
        message
    } else {
        "Browser pairing storage failed; allow site storage and try again.".into()
    }
}
