// Invoked only in the verified EA Desktop SPA. Qt owns all entitlement,
// content checks and download operations; no account data leaves that client.
async function raesirEa(input) {
    const manager = globalThis.webchannel?.objects?.GamesManager;
    if (!manager || typeof manager.getGameStatusV2 !== "function"
        || typeof manager.getQueuedOfferIds !== "function") throw new Error("Sign in to EA app first");
    const validId = id => typeof id === "string" && id.length > 0 && id.length <= 256 && !/[\x00-\x1f]/.test(id);
    const call = (name, ...args) => new Promise((resolve, reject) => {
        if (typeof manager[name] !== "function") return reject(new Error("Unsupported EA client interface"));
        const timer = setTimeout(() => reject(new Error("EA app is not ready")), 3000);
        manager[name](...args, result => { clearTimeout(timer); resolve(result); });
    });
    const status = async id => {
        const result = await call("getGameStatusV2", id);
        if (!result || typeof result !== "object" || Array.isArray(result)) throw new Error("Unsupported EA game status");
        const game = result.gameStatus && typeof result.gameStatus === "object" ? result.gameStatus : result;
        let info = result.installInfo && typeof result.installInfo === "object" ? result.installInfo : game;
        if (info.installStatusString === undefined) info = await call("getInstallInfo", id, "raesir");
        if (!info || typeof info !== "object") throw new Error("Unsupported EA installation status");
        const state = info.installStatusString ?? game.installStatusString;
        return { game, info, state, managed: result.managedBy === undefined || result.managedBy === "EA" };
    };
    if (input.action !== "snapshot") {
        if (!validId(input.id)) throw new Error("Invalid EA offer ID");
        if (input.action === "pause" || input.action === "resume") {
            const queued = await call("getQueuedOfferIds");
            if (!Array.isArray(queued) || !queued.includes(input.id)) throw new Error("This game is not in EA's download queue");
            // These are GamesManager's native SPA controls, not LSX's global
            // downloader-utilization setting: Pause=1, Resume=2, Cancel=3.
            await call("issueDownloadControlV2", { offerIds: [input.id], control: input.action === "pause" ? 1 : 2 });
            let changed = false;
            for (let i = 0; i < 10; i++) {
                await new Promise(resolve => setTimeout(resolve, 100));
                const current = await status(input.id);
                if (input.action === "pause" ? current.state === "PAUSED" : ["QUEUED", "ACTIVE"].includes(current.state)) { changed = true; break; }
            }
            if (!changed) throw new Error("EA did not confirm the download state");
        } else if (input.action === "update") {
            const existing = await call("getQueuedOfferIds");
            const current = await status(input.id);
            if (!Array.isArray(existing)) throw new Error("Unsupported EA download queue");
            if (!current.managed) throw new Error("Manage this game's update in its original client");
            if (!existing.includes(input.id) && current.game.updateavailable !== true && current.game.updaterequired !== true) throw new Error("EA has no update for this game");
            if (!existing.includes(input.id)) await call("initiateUpdateV3", { offerIds: [input.id], trigger: "raesir" });
        } else throw new Error("Unsupported EA download action");
    }
    const queued = await call("getQueuedOfferIds");
    if (!Array.isArray(queued) || !queued.every(validId) || queued.length > 4096) throw new Error("Unsupported EA download queue");
    const installed = await call("getAllInstalledOfferIds");
    if (!Array.isArray(installed) || !installed.every(validId) || installed.length > 4096) throw new Error("EA library is not ready");
    const ids = [...new Set([...queued, ...(input.checkUpdates ? installed : []), ...(input.watching || []).filter(id => installed.includes(id))])];
    const downloads = [], patches = [], completed = [];
    // Small bounded batches avoid flooding Qt on large libraries.
    for (let index = 0; index < ids.length; index += 8) {
        await Promise.all(ids.slice(index, index + 8).map(async id => {
            const current = await status(id);
            if (!current.managed) return;
            if (current.game.updateavailable === true || current.game.updaterequired === true) patches.push(id);
            if (!queued.includes(id)) {
                if (installed.includes(id) && current.state === "INSTALLED" && current.game.updateavailable === false && current.game.updaterequired === false) completed.push(id);
                return;
            }
            const progress = await call("downloadProgress", id);
            const downloaded = progress?.byteDownloaded, total = progress?.bytesTotal;
            const counters = Number.isSafeInteger(downloaded) && Number.isSafeInteger(total) && downloaded >= 0 && total > 0 && downloaded <= total;
            const phase = current.info.installPhaseString ?? current.game.installPhaseString;
            downloads.push({ id, state: current.state, phase, pausable: current.info.pausable === true,
                downloaded: counters ? downloaded : null, total: counters ? total : null });
        }));
    }
    return { ready: true, paused: false, patches: input.checkUpdates ? patches : null, downloads, completed };
}
