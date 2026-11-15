//! Transactional browser-local records, separate from encrypted pairing credentials.
//!
//! Both stores participate in every transaction. A successful CAS acknowledges the
//! transaction's completion, never an individual request, and broadcasts its revision.
use std::collections::BTreeMap;

use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(inline_js = r#"
const stores = ["records", "meta"];
const maxRevision = 18446744073709551615n;
let revisionChannel;

function revision(value) {
    if (value === undefined) return "0";
    if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value) ||
        BigInt(value) > maxRevision) {
        throw new Error("The local-state revision is damaged or unsupported.");
    }
    return value;
}

function openLocalState() {
    return new Promise((resolve, reject) => {
        let abandoned = false;
        const request = indexedDB.open("corral-local-state", 1);
        request.onupgradeneeded = () => {
            const database = request.result;
            for (const store of stores) {
                if (!database.objectStoreNames.contains(store)) {
                    database.createObjectStore(store);
                }
            }
        };
        request.onerror = () => reject(request.error || new Error("IndexedDB could not be opened."));
        request.onblocked = () => {
            abandoned = true;
            reject(new Error("Close other Corral tabs; they are blocking the local-state database."));
        };
        request.onsuccess = () => {
            const database = request.result;
            database.onversionchange = () => database.close();
            if (abandoned) database.close();
            else resolve(database);
        };
    });
}

function transact(database, mode, operation) {
    return new Promise((resolve, reject) => {
        // Strict durability requests a persistence flush before the completion event.
        const transaction = mode === "readwrite"
            ? database.transaction(stores, mode, { durability: "strict" })
            : database.transaction(stores, mode);
        let result;
        let failure;
        const abort = error => {
            failure = error;
            try { transaction.abort(); }
            catch (_) { reject(error); }
        };
        const guard = action => event => {
            try { action(event); }
            catch (error) { abort(error); }
        };
        transaction.oncomplete = () => resolve(result);
        transaction.onerror = event => {
            failure = failure || event.target.error || transaction.error;
        };
        transaction.onabort = () => reject(failure || transaction.error ||
            new Error("The local-state transaction was aborted."));
        try { result = operation(transaction, guard); }
        catch (error) { abort(error); }
    });
}

function notifyRevision(value) {
    // Notifications are hints: a failure after commit must not imply a failed CAS.
    if (typeof BroadcastChannel === "undefined") return;
    try {
        if (!revisionChannel) revisionChannel = new BroadcastChannel("corral-local-state");
        revisionChannel.postMessage({ revision: value });
    } catch (error) {
        console.warn("Local state committed, but revision notification failed.", error);
    }
}

export async function corral_local_read() {
    let database;
    try {
        database = await openLocalState();
        const snapshot = await transact(database, "readonly", (transaction, guard) => {
            const result = { revision: "0", records: [] };
            const meta = transaction.objectStore("meta").get("revision");
            meta.onsuccess = guard(() => { result.revision = revision(meta.result); });
            const request = transaction.objectStore("records").openCursor();
            request.onsuccess = guard(() => {
                const cursor = request.result;
                if (!cursor) return;
                if (typeof cursor.key !== "string" || typeof cursor.value !== "string") {
                    throw new Error("A local-state record is damaged or unsupported.");
                }
                result.records.push([cursor.key, cursor.value]);
                cursor.continue();
            });
            return result;
        });
        return JSON.stringify([snapshot.revision, snapshot.records]);
    } finally {
        if (database) database.close();
    }
}

export async function corral_local_compare_exchange(expected, bytes) {
    // Decode into owned JS strings before awaiting: bytes borrows wasm memory.
    // Values remain JSON text, preserving u64 integers and JSON null vs deletion.
    const changes = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    const expectedRevision = revision(expected);
    let database;
    try {
        database = await openLocalState();
        const result = await transact(database, "readwrite", (transaction, guard) => {
            const result = { matched: false, revision: null };
            const meta = transaction.objectStore("meta");
            const request = meta.get("revision");
            request.onsuccess = guard(() => {
                const current = revision(request.result);
                if (current !== expectedRevision) return;
                if (BigInt(current) === maxRevision) {
                    throw new Error("The local-state revision is exhausted.");
                }
                const records = transaction.objectStore("records");
                for (const [key, value] of changes) {
                    if (value === null) records.delete(key);
                    else records.put(value, key);
                }
                result.revision = (BigInt(current) + 1n).toString();
                meta.put(result.revision, "revision");
                result.matched = true;
            });
            return result;
        });
        if (result.matched) notifyRevision(result.revision);
        return result.matched;
    } finally {
        if (database) database.close();
    }
}
"#)]
extern "C" {
    fn corral_local_read() -> js_sys::Promise;
    fn corral_local_compare_exchange(expected: &str, bytes: &[u8]) -> js_sys::Promise;
}

pub(super) async fn read() -> Result<(u64, BTreeMap<String, serde_json::Value>), String> {
    let value = JsFuture::from(corral_local_read())
        .await
        .map_err(storage_error)?;
    let encoded = value
        .as_string()
        .ok_or_else(|| "Browser local-state storage returned an invalid snapshot.".to_string())?;
    let (revision, records): (String, Vec<(String, String)>) = serde_json::from_str(&encoded)
        .map_err(|error| format!("Could not decode browser local-state snapshot: {error}"))?;
    let revision = revision
        .parse::<u64>()
        .map_err(|error| format!("Could not decode browser local-state revision: {error}"))?;
    let records = records
        .into_iter()
        .map(|(key, value)| {
            serde_json::from_str(&value)
                .map(|value| (key, value))
                .map_err(|error| format!("Could not decode browser local-state record: {error}"))
        })
        .collect::<Result<_, _>>()?;
    Ok((revision, records))
}

pub(super) async fn compare_exchange(
    expected: u64,
    changes: BTreeMap<String, Option<serde_json::Value>>,
) -> Result<bool, String> {
    let changes = changes
        .into_iter()
        .map(|(key, value)| {
            value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map(|value| (key, value))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not encode browser local-state changes: {error}"))?;
    let bytes = serde_json::to_vec(&changes)
        .map_err(|error| format!("Could not encode browser local-state transaction: {error}"))?;
    JsFuture::from(corral_local_compare_exchange(&expected.to_string(), &bytes))
        .await
        .map_err(storage_error)?
        .as_bool()
        .ok_or_else(|| {
            "Browser local-state storage returned an invalid acknowledgement.".to_string()
        })
}

fn storage_error(error: JsValue) -> String {
    let detail = if let Some(error) = error.dyn_ref::<js_sys::Error>() {
        String::from(error.message())
    } else if let Some(message) = error.as_string() {
        message
    } else {
        "IndexedDB is unavailable or the transaction was aborted.".into()
    };
    format!("Browser local-state storage failed: {detail}")
}
