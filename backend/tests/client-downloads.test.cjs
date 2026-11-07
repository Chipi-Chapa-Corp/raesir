const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

function load(name, globals) {
    const context = vm.createContext({ setTimeout, clearTimeout, ...globals });
    vm.runInContext(fs.readFileSync(path.join(__dirname, '../src/client_downloads', name + '.js'), 'utf8'), context);
    return context;
}
function ubisoft() {
    const names = ['QueryGetDownloadQueue','QueryGetGamesWithPendingPatches','MessageDownloadProgress','MessageDownloadCompleted','MessagePauseDownload','MessageResumeDownload','MessageDownloadGame','MessageInstallGame','QueryGetProducts'];
    const messages = Object.fromEntries(names.map(name => [name, class { constructor(data) { this.type = name; Object.assign(this, data); if(name.startsWith('Query')) this.myQueryName=name; } }]));
    const queue = { myIsPaused: false, myDownloads: [
        { myId: 10, myGameId: 42, myState: 2, myIsActive: true, myType: 'DownloadType_Edison' },
        { myId: 11, myGameId: 43, myState: 0, myIsActive: false, myType: 'DownloadType_Legacy' },
        { myId: 12, myGameId: 42, myState: 2, myIsActive: false, myType: 'DownloadType_Additional' },
    ] };
    const routes = new Map(), sent = [];
    const sdk = { locRequestIds: new Map(), locQueryDeferrals: new Map(), next: 1 };
    let hang = false;
    let patches = { myProductIds: [42] }, products = { myProducts: [] };
    const context = load('ubisoft', { uplayUiMessage() {}, System: { import: async name => name==='@shared/upc/Upc' ? { default:sdk } : messages }, upc: {
        sendQuery(q) {
            const key = `${q.myQueryName}-${JSON.stringify(q)}`;
            q.myDispatchId=sdk.next++;
            sdk.locRequestIds.set(key,q.myDispatchId);
            sdk.locQueryDeferrals.set(q.myDispatchId,{fixture:true});
            return hang ? new Promise(()=>{}) : Promise.resolve(q.type === 'QueryGetDownloadQueue' ? queue : q.type === 'QueryGetProducts' ? products : patches);
        },
        sendMessage(message) {
            sent.push(message);
            if (message.type === 'MessagePauseDownload') queue.myIsPaused = true;
            if (message.type === 'MessageResumeDownload') queue.myIsPaused = false;
        },
        observeRoute(type) { return { subscribe(callback) { routes.set(new type().type, callback); } }; },
    } });
    return { queue, routes, sent, context, sdk, hang: value=>hang=value, patches: value => patches=value, products: value => products=value,
        run: input => context.raesirUbisoft({ action: 'snapshot', ...input }) };
}

test('Ubisoft observes byte progress, excludes DLC, and rejects unknown patch schemas', async () => {
    const client = ubisoft();
    await client.run({});
    client.routes.get('MessageDownloadProgress')({ myId: 10, myProgress: { myBytesDownloaded: 20, myBytesToDownload: 80 } });
    const snapshot = await client.run({ checkUpdates: true });
    assert.equal(snapshot.downloads.length, 2);
    assert.equal(snapshot.downloads[0].downloaded, 20);
    assert.deepEqual(Array.from(snapshot.patches), ['42']);
    client.patches({ myGames: [42], otherIds: [99] });
    assert.equal((await client.run({ checkUpdates: true })).patches, null);
});

test('Ubisoft cannot pause a different game through its global queue control', async () => {
    const client = ubisoft();
    await assert.rejects(client.run({ action: 'pause', id: '43' }), /not Ubisoft's active/);
    assert.equal(client.sent.length, 0);
    assert.equal((await client.run({ action: 'pause', id: '42' })).paused, true);
    assert.equal((await client.run({ action: 'resume', id: '42' })).paused, false);
});

test('Ubisoft queues updates without launching and does not submit duplicates', async () => {
    const client = ubisoft();
    await client.run({ action: 'update', id: '42' });
    assert.equal(client.sent.length, 0);
    await client.run({ action: 'update', id: '44' });
    assert.equal(client.sent[0].type, 'MessageDownloadGame');
    assert.equal(client.sent[0].mySetActive, false);
    await assert.rejects(client.run({ action: 'update', id: '1;window.close()' }), /Invalid Ubisoft/);
});

test('Ubisoft completion does not complete a newer active transfer for the same game', async () => {
    const client = ubisoft();
    await client.run({});
    client.routes.get('MessageDownloadCompleted')({ myId: 10 });
    assert.equal((await client.run({})).completed.length, 0);
    client.queue.myDownloads = [];
    assert.deepEqual(Array.from((await client.run({})).completed), ['42']);
});

function ea() {
    const queued = ['offer-a'], installed = ['offer-a','offer-b'];
    const states = {
        'offer-a': { gameStatus: { updateavailable: false, updaterequired: false }, installInfo: { installStatusString: 'ACTIVE', installPhaseString: 'DOWNLOAD', pausable: true } },
        'offer-b': { gameStatus: { updateavailable: true, updaterequired: false }, installInfo: { installStatusString: 'INSTALLED', pausable: false } },
    };
    const sent = [];
    const manager = {
        getGameStatusV2(id, callback) { callback(states[id]); },
        getQueuedOfferIds(callback) { callback(queued); },
        getAllInstalledOfferIds(callback) { callback(installed); },
        downloadProgress(id, callback) { callback({ bytesTotal: 100, byteDownloaded: 40 }); },
        issueDownloadControlV2(input, callback) {
            sent.push(input);
            for (const id of input.offerIds) states[id].installInfo.installStatusString = input.control===1 ? 'PAUSED' : 'ACTIVE';
            callback();
        },
        initiateUpdateV3(input, callback) { sent.push(input); queued.push(...input.offerIds); callback(); },
    };
    const context = load('ea', { webchannel: { objects: { GamesManager: manager } } });
    return { queued, installed, states, sent, manager, run: input => context.raesirEa({ action: 'snapshot', ...input }) };
}

test('EA reads explicit update flags and live progress from its native queue', async () => {
    const client = ea(), snapshot = await client.run({ checkUpdates: true });
    assert.deepEqual(Array.from(snapshot.patches), ['offer-b']);
    assert.equal(snapshot.downloads[0].downloaded, 40);
    assert.equal(snapshot.downloads[0].pausable, true);
    assert.equal(snapshot.completed.length, 0);
});

test('EA pause/resume targets only the requested queued offer', async () => {
    const client = ea();
    await assert.rejects(client.run({ action: 'pause', id: 'offer-b' }), /not in EA's download queue/);
    await client.run({ action: 'pause', id: 'offer-a' });
    assert.deepEqual(Array.from(client.sent[0].offerIds), ['offer-a']);
    assert.equal(client.sent[0].control, 1);
    await client.run({ action: 'resume', id: 'offer-a' });
    assert.equal(client.sent[1].control, 2);
});

test('EA updates require native update evidence and repeated requests do not duplicate jobs', async () => {
    const client = ea();
    await client.run({ action: 'update', id: 'offer-b' });
    await client.run({ action: 'update', id: 'offer-b' });
    assert.equal(client.sent.length, 1);
    client.queued.length = 0;
    await assert.rejects(client.run({ action: 'update', id: 'offer-a' }), /no update/);
});

test('EA completion requires installed, unqueued, explicitly up-to-date content', async () => {
    const client = ea();
    client.states['offer-b'].gameStatus.updateavailable = false;
    assert.deepEqual(Array.from((await client.run({ checkUpdates: true })).completed), ['offer-b']);
    delete client.states['offer-b'].gameStatus.updaterequired;
    assert.equal((await client.run({ checkUpdates: true })).completed.length, 0);
});

test('Unknown and signed-out EA interfaces fail without sending download requests', async () => {
    const context = load('ea', {});
    await assert.rejects(context.raesirEa({ action: 'update', id: 'offer-a' }), /Sign in/);
    const client = ea();
    client.manager.getQueuedOfferIds = callback => callback({ unexpected: ['offer-a'] });
    await assert.rejects(client.run({}), /Unsupported EA download queue/);
    assert.equal(client.sent.length, 0);
});

test('Ubisoft query timeout releases only the bridge request and recovers after sign-in', async () => {
    const client=ubisoft();
    client.sdk.locRequestIds.set('UI query',999);
    client.sdk.locQueryDeferrals.set(999,{belongsToClient:true});
    client.hang(true);
    await assert.rejects(client.run({}),/client is not ready/);
    assert.equal(client.sdk.locRequestIds.size,1);
    assert.equal(client.sdk.locQueryDeferrals.size,1);
    assert.equal(client.sdk.locRequestIds.get('UI query'),999);
    client.hang(false);
    assert.equal((await client.run({})).downloads.length,2);
    assert.equal(client.sdk.locRequestIds.size,1);
});
