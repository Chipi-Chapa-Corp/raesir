# Raesir

Minimal Tauri 2 + Rust + Svelte 5 Steam inventory prototype. The UI is a bullet list of titles and their ownership/installation states.

## Run

Install Node.js, Rust, and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). On Fedora the desktop build needs `gtk3-devel`, `webkit2gtk4.1-devel`, and `librsvg2-devel`.

```sh
npm ci
npm run tauri dev
```

Build a standalone development binary without creating installers:

```sh
npm run tauri build -- --debug --no-bundle
```

The Linux binary is `src-tauri/target/debug/raesir`.

## What works

- Finds native and Flatpak Steam roots on Linux; uses the Steam registry path and standard locations on Windows.
- Reads modern and older `libraryfolders.vdf` layouts, and `appmanifest_*.acf` files across libraries.
- Lists installed, incomplete, missing-folder, owned-only, and previously seen/unavailable states. Shared Steam runtimes are filtered out.
- Merges installations by Steam AppID, retaining each path and managing OS. Installation OS describes the Steam installation, not whether the payload is a Windows executable used through Proton.
- Enumerates partitions with `lsblk` on Linux and PowerShell's Storage module on Windows, including other partitions on the same physical disk.
- Inspects readable mounts for Windows/Linux OS markers and known Steam locations, including Linux home and common Btrfs subvolume layouts.
- Accepts additional Steam folders, library folders, or mounted OS roots, with an optional OS hint. Explicit path mappings translate paths such as `D:` to `/mnt/games`, or `/home` to `L:\home`.
- Remembers previous installations when a library is inaccessible. A successful scan of an empty library can establish removal; a failed or corrupt scan cannot.
- Reports unavailable partitions and actionable filesystem-tool warnings. On Windows a readable Btrfs volume can be supplied through WinBtrfs. Unmounted Linux GPT partitions are reported even without filesystem identification; raw superblock probing can identify Btrfs/ext4 when existing permissions allow it.

Storage discovery never changes partitions. The separate **Mount read-only** action only runs after an explicit click. An NTFS/Linux partition is only a candidate until its OS or Steam files can be read. Extra folders and path mappings are remembered in the app's local webview storage.

## Ownership

Installed manifests do **not** establish account ownership. Offline scans explicitly show **Ownership unknown**.

Expand **Steam library & ownership → Use a Steam Web API key instead** to query Valve's `GetOwnedGames` endpoint with a SteamID64 and Steam Web API key. Game details must be visible to that API. The key is used only for that request, sent to `api.steampowered.com`, and never saved. Titles returned by Steam are cached for the one selected account. Ownership reflects that last successful lookup, including played free games returned by the API. An inaccessible/private library does not replace the previous owned list with an empty list.

Results are stored in Tauri's application data directory (`dev.raesir.launcher`): `inventory.json` and `owned.json`. **Forget owned list** removes the account-library cache while preserving installation discovery. `RAESIR_DATA_DIR` overrides the directory for isolated tests.

## Managed Steam session (experimental)

On Linux x86_64, **Read running Steam library** queries an already-running Steam session without starting another client or taking ownership of its lifetime. If Steam is not running, **Start Steam & read library** starts the selected detected native/Flatpak client with `-silent`. Steam reuses its own saved login. Finish any Steam login/Guard prompt normally, then retry the lookup. Raesir never reads or copies passwords, cookies or refresh tokens.

A disposable subprocess loads the selected client's `linux64/steamclient.so`, connects to its global user using versioned interfaces, and checks game/demo/mod IDs from `appcache/appinfo.vdf` against `ISteamApps::BIsSubscribedApp`. Cached metadata alone is never used as ownership proof. The helper has a timeout and crashes cannot take down the Tauri UI.

This is a client-version/runtime-dependent experiment, not a supported general-purpose Valve launcher API. It can only return games present in the client's metadata cache. Shared/temporary licenses may be included, so its results say **In Steam library**, while the Web API results retain **Owned**. These methods replace the one cached account list, rather than combining accounts.

An already-running Steam client is borrowed for the lookup and remains running when Raesir closes, including after a failed lookup. **Stop managed Steam** is disabled for borrowed sessions. The selected installation still supplies the IPC library and cached catalog; select the installation corresponding to your running client.

For Steam started by Raesir, native/Flatpak bootstrap and Steam/helper process identities are tracked; normal app close waits for graceful `-shutdown`, then terminates only recorded owned processes if needed. If Steam still cannot be stopped, Raesir stays open and reports the error. **Stop managed Steam** also works after a failed login/import. Existing unrelated Steam processes are never shut down. Forced termination of Raesir or a system crash can bypass normal exit cleanup.

The bridge currently supports Linux x86_64 only. Windows still has the Web API option. Run the app in a runtime able to launch and communicate with the installed Steam client; Steam inside another sandbox/container can be incompatible. No arbitrary AppID, injected Steam plugin, copied auth token or web-debugging port is used.

## Mounting with OS authorization

For unmounted NTFS/ext4/Btrfs volumes on Linux, expand **Storage & scan warnings** and click **Mount read-only**. The backend re-enumerates the selected volume and checks the UDisks device/filesystem before calling `org.freedesktop.UDisks2.Filesystem.Mount`. Your desktop's Polkit agent may ask for approval/an administrator password; Raesir never receives that password. There is no sudo prompt or writable fallback.

NTFS requests `ro`, ext4 `ro,noload`, and Btrfs `ro,nologreplay`. UDisks must allow those options. `/etc/fstab` devices are refused because UDisks can ignore caller options for them; mount/configure those read-only through the OS first. Encrypted devices still require unlocking using OS tools. Successful mounts trigger a scan and remain mounted until you unmount them using your OS tools.

UDisks and a desktop authorization agent are required. On Toolbx, the explicit mount action can use the exposed host system-bus socket at `/run/host/run/dbus/system_bus_socket` if the ordinary system bus is unavailable. Existing explicit bus configuration is respected. `RAESIR_DISABLE_HOST_BRIDGE=1` disables that fallback for isolated diagnostics. Host mounts exposed under `/run/host/run/media`, `/run/host/mnt`, `/run/host/media` or `/run/host/var/mnt` are scanned too.

## Current boundaries

This increment discovers games and optionally starts a managed Steam client; game launching is still outside its scope.

Other-OS scanning currently requires readable filesystem paths. It does not bundle WinBtrfs, an ext4 reader, LUKS support, or an elevated storage helper. Missing/unavailable support is reported instead of treating the partition as empty. On Windows, an unmounted Linux partition may need administrator read access before its exact filesystem can be identified. WSL `--mount` cannot attach the disk running Windows, even for a different partition.

Automatic directory discovery is bounded to known locations. Custom Steam roots, unusual mount layouts, separate home partitions and unresolved foreign library paths can be added manually. Drive-letter mapping is not guessed when several mounted volumes match. Linux/Windows data files are not automatically shared across boots; scans can read the other installation directly when its partition is accessible.

The Windows scanner is cross-checked for compilation; Windows runtime/storage behavior still needs validation on Windows hardware. Linux filesystem discovery and the Tauri UI are exercised locally. Steam IPC is tested against a compiled native ABI fixture, process cleanup against test child processes, and mount requests against a fake UDisks service on a dedicated private D-Bus. Live Steam login/auth and real partition mounting/Polkit prompts have not been exercised, to avoid taking over running user clients or mounting user partitions during agent-owned diagnostics.

## Verification

```sh
npm run check
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Read-only JSON diagnostics without a GUI, writes, or network requests:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --no-default-features --bin raesir-scan
# Additional accessible OS roots / Steam folders:
cargo run --manifest-path src-tauri/Cargo.toml --no-default-features --bin raesir-scan -- /mnt/windows
```

Agent-owned GUI tests must use the private display/D-Bus helper described in the user's `isolated-gui` skill. Do not launch diagnostics on the user's desktop. Use fresh application-data and XDG directories, set `RAESIR_DISABLE_HOST_BRIDGE=1`, and do not request real mounts or start the user's Steam during GUI tests.
