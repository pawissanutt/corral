// Real-browser IndexedDB regressions, with no package or build dependencies.
// Serve apps/corral-app on localhost, then in that origin's browser console:
// await (await import('/src/client/durable_web_tests.mjs')).runBrowserDurabilityRegressions()
// Each run uses a disposable database/channel, never the app's saved state.

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

async function rejected(promise, message) {
    let failed = false;
    try { await promise; }
    catch (_) { failed = true; }
    assert(failed, message);
}

function encode(changes) {
    return new TextEncoder().encode(JSON.stringify(changes));
}

async function snapshot(client) {
    const [revision, records] = JSON.parse(await client.corral_local_read());
    return { revision, records: new Map(records) };
}

async function seedRevision(name, revision) {
    const database = await new Promise((resolve, reject) => {
        const request = indexedDB.open(name, 1);
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
    });
    try {
        await new Promise((resolve, reject) => {
            const transaction = database.transaction('meta', 'readwrite');
            transaction.oncomplete = resolve;
            transaction.onabort = () => reject(transaction.error);
            transaction.objectStore('meta').put(revision, 'revision');
        });
    } finally {
        database.close();
    }
}

export async function runBrowserDurabilityRegressions() {
    const response = await fetch(new URL('./durable_web.rs', import.meta.url));
    assert(response.ok, 'Could not fetch the production Rust bridge source');
    const match = (await response.text()).match(/inline_js = r#"([\s\S]*?)"#\)\]/);
    assert(match, 'Could not extract the production inline JavaScript');
    const name = `corral-durability-test-${crypto.randomUUID()}`;
    const channelName = `${name}-revision`;
    const channels = [];
    const fixtureKey = `__${name}`;
    // Production code is imported unchanged. Only its ambient database/channel
    // factories are scoped to this disposable fixture.
    globalThis[fixtureKey] = {
        open: () => indexedDB.open(name, 1),
        channel: class extends BroadcastChannel {
            constructor() {
                super(channelName);
                channels.push(this);
            }
        },
    };
    const source = `const indexedDB = {open: globalThis[${JSON.stringify(fixtureKey)}].open};
const BroadcastChannel = globalThis[${JSON.stringify(fixtureKey)}].channel;
${match[1]}`;
    const urls = [];
    const loadClient = async () => {
        const url = URL.createObjectURL(new Blob([source], { type: 'text/javascript' }));
        urls.push(url);
        return import(url);
    };
    let listener;
    let restorePut;
    const passed = [];
    try {
        // Independent module instances use independent database connections,
        // reproducing competing tabs without a fake IndexedDB implementation.
        const first = await loadClient();
        const second = await loadClient();
        const initial = await snapshot(first);
        assert(initial.revision === '0' && initial.records.size === 0,
            'A new database must have an empty revision-zero snapshot');
        const contenders = [
            ['one', '{"value":1}'],
            ['two', '{"value":2}'],
        ];
        const results = await Promise.all([
            first.corral_local_compare_exchange('0', encode([contenders[0]])),
            second.corral_local_compare_exchange('0', encode([contenders[1]])),
        ]);
        assert(results.filter(Boolean).length === 1, 'Only one stale-revision contender may commit');
        const loser = results[0] ? 1 : 0;
        assert(await second.corral_local_compare_exchange('1', encode([contenders[loser]])),
            'Retrying the losing patch against the latest revision must commit');
        const merged = await snapshot(first);
        assert(merged.revision === '2' && merged.records.size === 2 &&
            contenders.every(([key, value]) => merged.records.get(key) === value),
            'A retried per-record patch must preserve the other tab\'s records');
        passed.push('competing tabs preserve unrelated records and reject stale revisions');

        const borrowed = encode([
            ['one', null],
            ['null', 'null'],
            ['integer', '18446744073709551615'],
        ]);
        const pending = first.corral_local_compare_exchange('2', borrowed);
        borrowed.fill(0);
        assert(await pending, 'Mutating borrowed wasm bytes after return must not corrupt a commit');
        const values = await snapshot(second);
        assert(values.revision === '3' && !values.records.has('one') &&
            values.records.get('null') === 'null' &&
            values.records.get('integer') === '18446744073709551615' &&
            values.records.has('two'),
            'JSON null, deletion, exact integers and unchanged records must remain distinct');
        passed.push('borrowed input lifetime, null/deletion distinction and integer precision');

        // Abort after a successful put, before commit. Resolving a request's
        // success rather than the transaction completion would falsely acknowledge it.
        const originalPut = IDBObjectStore.prototype.put;
        let injected = false;
        IDBObjectStore.prototype.put = function (...args) {
            const request = originalPut.apply(this, args);
            if (!injected && this.transaction.db.name === name && this.name === 'records') {
                injected = true;
                const transaction = this.transaction;
                request.addEventListener('success', () => transaction.abort(), { once: true });
            }
            return request;
        };
        restorePut = () => { IDBObjectStore.prototype.put = originalPut; };
        await rejected(first.corral_local_compare_exchange('3', encode([
            ['two', '"must roll back"'], ['partial', 'true'],
        ])), 'An aborted transaction must not return a successful acknowledgement');
        restorePut();
        restorePut = undefined;
        const rolledBack = await snapshot(second);
        assert(injected && rolledBack.revision === '3' &&
            JSON.stringify([...rolledBack.records]) === JSON.stringify([...values.records]),
            'Abort must roll back every changed record and the revision');
        passed.push('late abort rejects acknowledgement and rolls back the complete patch');

        const concurrent = await Promise.all([
            snapshot(second),
            first.corral_local_compare_exchange('3', encode([
                ['atomic-a', '"a"'], ['atomic-b', '"b"'],
            ])),
            snapshot(second),
        ]);
        assert(concurrent[1], 'The multi-record patch must commit');
        for (const observed of [concurrent[0], concurrent[2]]) {
            const hasA = observed.records.get('atomic-a') === '"a"';
            const hasB = observed.records.get('atomic-b') === '"b"';
            assert(hasA === hasB && observed.revision === (hasA ? '4' : '3'),
                'A concurrent read must not mix records or revision across transactions');
        }
        passed.push('concurrent reads observe a single consistent multi-record revision');

        listener = new BroadcastChannel(channelName);
        const notified = new Promise((resolve, reject) => {
            const timer = setTimeout(() => reject(new Error('Committed revision was not broadcast')), 3000);
            listener.onmessage = event => {
                clearTimeout(timer);
                resolve(event.data);
            };
        });
        // Revisions are full u64s, not JavaScript Number counters.
        await seedRevision(name, '9007199254740993');
        assert(await first.corral_local_compare_exchange('9007199254740993', encode([])),
            'An empty patch must still commit a new exact revision');
        const event = await notified;
        assert(event.revision === '9007199254740994', 'Broadcast must carry the exact committed revision');
        assert((await snapshot(second)).revision === event.revision,
            'A notified revision must already be readable by another connection');
        await seedRevision(name, '18446744073709551615');
        await rejected(first.corral_local_compare_exchange('18446744073709551615', encode([
            ['overflow', 'true'],
        ])), 'Revision overflow must fail, not wrap');
        const exhausted = await snapshot(second);
        assert(exhausted.revision === '18446744073709551615' && !exhausted.records.has('overflow'),
            'Exhausted revisions must not mutate records');
        passed.push('committed-revision broadcasts, exact u64 revisions and overflow rollback');
        return passed;
    } finally {
        if (restorePut) restorePut();
        if (listener) listener.close();
        for (const channel of channels) channel.close();
        for (const url of urls) URL.revokeObjectURL(url);
        delete globalThis[fixtureKey];
        await new Promise((resolve, reject) => {
            const request = indexedDB.deleteDatabase(name);
            request.onsuccess = resolve;
            request.onerror = () => reject(request.error);
            request.onblocked = () => reject(new Error('Fixture database cleanup was blocked'));
        });
    }
}
