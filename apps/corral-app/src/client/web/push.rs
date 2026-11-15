//! This inline module is also the service worker, so NSH ships exactly the same
//! script in development and hashed static packages; no root-scope interception.
use corral_protocol::device::PushSubscription;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(inline_js = r#"
const isWorker = typeof document === 'undefined' && typeof self.registration !== 'undefined';
if (isWorker) {
    self.addEventListener('push', event => {
        // Ignore payloads: notifications reveal no task, kind, count or workspace.
        event.waitUntil(self.registration.showNotification('Corral', {
            body: 'Something needs you', tag: 'corral-attention',
            data: { app: new URL(self.location.href).searchParams.get('app') }
        }));
    });
    self.addEventListener('notificationclick', event => {
        event.notification.close();
        event.waitUntil((async () => {
            const target = new URL(event.notification.data?.app || '/', self.location.origin);
            if (target.origin !== self.location.origin) return;
            const clients = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
            const existing = clients.find(client => {
                const url = new URL(client.url);
                return url.origin === target.origin && url.pathname === target.pathname;
            });
            if (existing) await existing.focus();
            else await self.clients.openWindow(target.href);
        })());
    });
}

function available() {
    if (!globalThis.isSecureContext || !('serviceWorker' in navigator) ||
        !('PushManager' in globalThis) || !('Notification' in globalThis)) {
        throw new Error('Web Push needs a secure browser with service workers and notifications.');
    }
}

function workerUrl() {
    const url = new URL(import.meta.url);
    if (url.origin !== location.origin) {
        throw new Error('Web Push requires same-origin hosting; CDN-hosted worker scripts cannot register.');
    }
    // Never retain fragments (pairing tokens) or query strings in worker metadata.
    url.searchParams.set('app', location.origin + location.pathname);
    url.searchParams.set('corral-push', '1');
    return url;
}

export async function corralPushPermission() {
    available();
    const permission = Notification.permission === 'granted'
        ? 'granted' : await Notification.requestPermission();
    if (permission !== 'granted') throw new Error('Browser notification permission was not granted.');
}

let pendingPermission;
export function corralPushBeginPermission() {
    pendingPermission = corralPushPermission();
    pendingPermission.catch(() => {});
}
export function corralPushAwaitPermission() {
    const permission = pendingPermission || corralPushPermission();
    pendingPermission = undefined;
    return permission;
}

export async function corralPushEnable(publicKey) {
    await corralPushPermission();
    const encoded = publicKey.replace(/-/g, '+').replace(/_/g, '/');
    const key = Uint8Array.from(atob(encoded), ch => ch.charCodeAt(0));
    if (key.length !== 65 || key[0] !== 4) throw new Error('The daemon VAPID public key is invalid.');
    const registration = await navigator.serviceWorker.register(workerUrl().href, { type: 'module' });
    if (!registration.active) {
        const worker = registration.installing || registration.waiting;
        if (!worker) throw new Error('The push worker did not install.');
        await new Promise((resolve, reject) => {
            const changed = () => {
                if (worker.state === 'activated') { worker.removeEventListener('statechange', changed); resolve(); }
                else if (worker.state === 'redundant') { worker.removeEventListener('statechange', changed); reject(new Error('The push worker failed to activate.')); }
            };
            worker.addEventListener('statechange', changed);
            changed();
        });
    }
    let subscription = await registration.pushManager.getSubscription();
    if (subscription) {
        const previous = subscription.options.applicationServerKey;
        if (!previous || !Array.from(new Uint8Array(previous)).every((byte, index) => byte === key[index]) || previous.byteLength !== key.length) {
            throw new Error('This browser already has push for another daemon key. Disable that subscription before switching daemons.');
        }
    } else {
        subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
    }
    const json = subscription.toJSON();
    if (!json.endpoint || !json.keys?.p256dh || !json.keys?.auth) throw new Error('The browser returned an incomplete push subscription.');
    return JSON.stringify({ endpoint: json.endpoint, p256dh: json.keys.p256dh, auth: json.keys.auth });
}

export async function corralPushDisable() {
    available();
    const app = location.origin + location.pathname;
    // NSH package hashes change the script directory. Find our older scopes too;
    // never remove a host site's unrelated service worker or subscription.
    for (const registration of await navigator.serviceWorker.getRegistrations()) {
        const worker = registration.active || registration.waiting || registration.installing;
        if (!worker) continue;
        const url = new URL(worker.scriptURL);
        if (url.origin !== location.origin || url.searchParams.get('corral-push') !== '1' || url.searchParams.get('app') !== app) continue;
        const subscription = await registration.pushManager.getSubscription();
        if (subscription && !(await subscription.unsubscribe())) throw new Error('The browser could not remove its push subscription.');
        if (!(await registration.unregister())) throw new Error('The browser could not remove its push worker.');
    }
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = corralPushBeginPermission)]
    fn begin_permission_js();
    #[wasm_bindgen(js_name = corralPushAwaitPermission)]
    fn permission_js() -> js_sys::Promise;
    #[wasm_bindgen(js_name = corralPushEnable)]
    fn enable_js(public_key: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = corralPushDisable)]
    fn disable_js() -> js_sys::Promise;
}

fn error(value: JsValue) -> String {
    value
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| value.as_string())
        .unwrap_or_else(|| "Browser push operation failed.".into())
}

/// Start synchronously inside the opt-in gesture, before queueing client work.
pub(crate) fn begin_permission() {
    begin_permission_js();
}

/// Await the gesture's permission decision before requesting the daemon key.
pub(crate) async fn request_permission() -> Result<(), String> {
    JsFuture::from(permission_js())
        .await
        .map(|_| ())
        .map_err(error)
}

pub(crate) async fn enable(vapid_public: &str) -> Result<PushSubscription, String> {
    let json = JsFuture::from(enable_js(vapid_public))
        .await
        .map_err(error)?
        .as_string()
        .ok_or_else(|| "The browser returned no push subscription.".to_owned())?;
    serde_json::from_str(&json)
        .map_err(|_| "The browser returned an invalid push subscription.".into())
}

pub(crate) async fn disable() -> Result<(), String> {
    JsFuture::from(disable_js())
        .await
        .map(|_| ())
        .map_err(error)
}
