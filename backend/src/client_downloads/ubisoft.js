// Evaluated only in the selected Ubisoft client's local native popup context.
// The installed client supplies the messaging classes and performs entitlement,
// install-directory, license and download checks itself.
async function exposedGamesUbisoft(input) {
    if (typeof uplayUiMessage !== "function" || !globalThis.upc?.sendQuery || !globalThis.System?.import) {
        throw new Error("Unsupported Ubisoft client interface");
    }
    const messages = await System.import("@shared/upc/messages/Download.message");
    const sdk = (await System.import("@shared/upc/Upc")).default;
    if (!sdk?.locRequestIds?.get || !sdk?.locQueryDeferrals?.delete) throw new Error("Unsupported Ubisoft query interface");
    const query = request => {
        // The SDK caches requests before replacing myDispatchId. Its own
        // queries start at -1; 0 gives this bridge a separate cache entry.
        // Clean up only our entry after a timeout, so signing in later retries
        // the native query instead of reusing an eternally pending promise.
        request.myDispatchId = 0;
        const key = `${request.myQueryName}-${JSON.stringify(request)}`;
        let timer;
        return Promise.race([
            upc.sendQuery(request),
            new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("Ubisoft client is not ready")), 3500); }),
        ]).finally(() => {
            clearTimeout(timer);
            if (request.myDispatchId > 0 && sdk.locRequestIds.get(key) === request.myDispatchId) {
                sdk.locRequestIds.delete(key);
                sdk.locQueryDeferrals.delete(request.myDispatchId);
            }
        });
    };
    const validId = id => /^(?:[1-9][0-9]{0,9})$/.test(String(id)) && Number(id) <= 4294967295;
    const stateKey = "__exposedGamesDownloads1";
    if (!globalThis[stateKey]) {
        const cache = { progress: new Map(), completed: new Set(), games: new Map() };
        upc.observeRoute(messages.MessageDownloadProgress).subscribe(message => {
            if (validId(message.myId) && cache.progress.size < 4096) {
                cache.progress.set(String(message.myId), message.myProgress);
            }
        });
        upc.observeRoute(messages.MessageDownloadCompleted).subscribe(message => {
            if (validId(message.myId) && cache.completed.size < 4096) cache.completed.add(String(message.myId));
        });
        globalThis[stateKey] = cache;
    }
    const cache = globalThis[stateKey];
    const readQueue = async () => {
        const result = await query(new messages.QueryGetDownloadQueue());
        if (!Array.isArray(result.myDownloads) || typeof result.myIsPaused !== "boolean") {
            throw new Error("Unsupported Ubisoft download queue");
        }
        return result;
    };
    let queue = await readQueue();
    for (const row of queue.myDownloads) {
        if (validId(row.myId) && validId(row.myGameId) && cache.games.size < 4096) cache.games.set(String(row.myId), String(row.myGameId));
    }
    if (input.action !== "snapshot") {
        if (!validId(input.id)) throw new Error("Invalid Ubisoft game ID");
        const gameId = Number(input.id);
        if (input.action === "pause" || input.action === "resume") {
            // The native pause message controls the active queue. Never let a
            // button on an unrelated queued card pause the current game.
            const active = queue.myDownloads.find(row => row.myIsActive === true);
            if (!active || String(active.myGameId) !== String(input.id)) {
                throw new Error("This is not Ubisoft's active download");
            }
            upc.sendMessage(input.action === "pause" ? new messages.MessagePauseDownload() : new messages.MessageResumeDownload());
            const expected = input.action === "pause";
            for (let i = 0; i < 12; i++) {
                await new Promise(resolve => setTimeout(resolve, 100));
                queue = await readQueue();
                if (queue.myIsPaused === expected) break;
            }
            if (queue.myIsPaused !== expected) throw new Error("Ubisoft did not change the download state");
        } else if (input.action === "update") {
            if (!queue.myDownloads.some(row => String(row.myGameId) === String(input.id))) {
                upc.sendMessage(new messages.MessageDownloadGame({ myGameId: gameId, mySetActive: false }));
            }
        } else if (input.action === "install") {
            const installation = await System.import("@shared/upc/messages/Install.message");
            upc.sendMessage(new installation.MessageInstallGame({ myGameId: gameId }));
        } else {
            throw new Error("Unsupported Ubisoft action");
        }
    }
    let patches = null;
    if ((input.checkUpdates || input.watching?.length) && messages.QueryGetGamesWithPendingPatches) {
        const result = await query(new messages.QueryGetGamesWithPendingPatches()).catch(() => ({}));
        // Native QueryResultGetGamesWithPendingPatches serializes product IDs,
        // independently of download IDs and request-routing metadata.
        if (Array.isArray(result.myProductIds) && result.myProductIds.every(validId)) {
            patches = result.myProductIds.map(String);
        }
    }
    let installed = [];
    if (input.checkUpdates || input.watching?.length) {
        const products = await System.import("@shared/upc/messages/Products.message");
        const result = await query(new products.QueryGetProducts()).catch(() => ({}));
        const games = Object.values(result.myProducts || {}).map(product => product.myGame).filter(Boolean);
        installed = games.filter(game => validId(game.myId)
            && (game.myState === "InstallState_Playable" || game.myState === 3)
            && game.myIsDownloadNeeded === false).map(game => String(game.myId));
    }
    const live = new Set(queue.myDownloads.filter(row => row.myState !== 7).map(row => String(row.myGameId)));
    const completed = [...new Set([...cache.completed].map(id => cache.games.get(id)).filter(validId).concat(installed))]
        .filter(id => !live.has(id) && (patches === null ? [...cache.completed].some(download => cache.games.get(download) === id) : !patches.includes(id)));
    return {
        ready: true,
        patches,
        paused: queue.myIsPaused,
        completed,
        downloads: queue.myDownloads.filter(row => validId(row.myGameId)
            && ["DownloadType_Edison", "DownloadType_EdisonThirdParty", "DownloadType_Legacy"].includes(row.myType)).map(row => {
            const progress = cache.progress.get(String(row.myId)) || row.myProgress || {};
            const downloaded = progress.myBytesDownloaded;
            const total = progress.myBytesToDownload;
            const counters = Number.isSafeInteger(downloaded) && Number.isSafeInteger(total)
                && downloaded >= 0 && total > 0 && downloaded <= total;
            return {
                id: String(row.myGameId), download_id: String(row.myId),
                state: row.myState, active: row.myIsActive === true, kind: row.myType,
                downloaded: counters ? downloaded : null, total: counters ? total : null,
            };
        }),
    };
}
