# Raesir

Minimal Tauri 2 + Rust + Svelte 5 Steam and Epic library prototype.

## Run

Install Node.js, Rust and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
On Fedora the desktop build needs `gtk3-devel`, `webkit2gtk4.1-devel`, and `librsvg2-devel`.

```sh
npm ci
npm run tauri dev
```

```sh
npm run tauri build -- --debug --no-bundle
```

The Linux binary is `src-tauri/target/debug/raesir`.

## Interface

The app scans known Steam/Epic locations automatically and displays one bullet list of titles, stores, and installation states. There are no manual folder, mapping, account-key, partition-details, or metadata controls. Old saved manual folder settings are not restored.

Search combines with five filters: **Installed**, **Not installed**, **On a different OS**, **Steam**, **Epic**. Installed means a ready installation on either OS. Not installed excludes games installed on another OS. On a different OS is an additional constraint, so selecting it always narrows results. Installed/not installed form one facet (both checked or both unchecked means either status); Steam/Epic form another. Search, status, OS location and store combine with AND. Unavailable historical foreign installations remain visible under On a different OS.

Unreadable mount candidates appear as OS banners with **Mount**. An unmounted filesystem cannot prove which OS is installed, so unverified candidates say Possible Windows/Linux installation detected. After mounting, readable OS markers establish the OS and known libraries are scanned automatically. No raw paths or manifest IDs are displayed in the game list.

## Steam

Native and Flatpak installations are detected on Linux; registry/default locations are checked on Windows. Manifests and libraryfolders identify installations, not ownership. Steam AppIDs remain separate from Epic IDs.

On Linux x86_64, startup and Rescan automatically query ownership through Steam's logged-in client. If Steam is already running, Raesir borrows its session without taking over shutdown. Otherwise Raesir starts Steam minimized using its existing login and stops only that managed session on exit. Finish login/Steam Guard in Steam and rescan if needed.

The client-auth bridge is experimental. A disposable subprocess loads the installed `steamclient.so` and checks cached game/demo/mod IDs using versioned client interfaces. No passwords, cookies or refresh tokens are copied. Cached metadata bounds the list; shared/temporary licenses can be included. A failed lookup keeps the last successful Raesir snapshot. Windows client-auth lookup and cross-sandbox client communication remain unsupported/unverified.

Normal close waits for managed Steam cleanup. Forced app termination or a system crash can bypass cleanup. Existing external Steam sessions are left running.

## Epic

Scanning reads official launcher `.item` manifests and `LauncherInstalled.dat`, Legendary installed/asset/metadata records, and Heroic library caches. Native and Flatpak Heroic profiles, legacy Legendary profiles, common Wine/Lutris/Bottles prefixes, and detected Steam Proton prefixes are checked. Mounted Windows/Linux roots and common home/Btrfs layouts are checked automatically. Discovery is bounded to known locations.

Heroic external-client installation markers are retained without inventing payload paths. Equal titles across stores remain separate; equal Epic AppNames merge their installations across OSes. Standalone DLC and Unreal/Marketplace content are filtered when metadata identifies them. Missing or corrupt sources preserve previous installations as unavailable; a valid empty source can confirm removal.

**Refresh Epic** queries every detected local Legendary/Heroic profile using the installed [Legendary CLI](https://github.com/derrod/legendary). Sign in using Heroic or `legendary auth` first. Legendary must be on PATH, or set `RAESIR_LEGENDARY_BINARY` to its executable. Raesir invokes `list --json --third-party --force-refresh`; Legendary handles its login refresh and cache writes. Foreign profiles are read-only. The CLI lookup has a timeout and is cancelled on normal exit. No Epic/Heroic client is started or stopped.

Library state reflects cached account results, possibly from multiple profiles; installed files alone do not prove ownership. Raesir does not decode official legacy/Early Preview account caches or invoke `legendary auth --import`, which can log the official launcher out. With only the official launcher, installation records are available; Heroic/Legendary supplies uninstalled account titles.

Formats are checked against upstream [Legendary models](https://github.com/derrod/legendary/blob/master/legendary/models/game.py), [Epic manifest models](https://github.com/derrod/legendary/blob/master/legendary/models/egl.py), [Heroic profile locations](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/blob/main/src/backend/storeManagers/legendary/constants.ts), and [Playnite's Epic reader](https://github.com/JosefNemec/PlayniteExtensions/blob/master/source/Libraries/EpicLibrary/EpicLauncher.cs).

## Mount lifecycle

On Linux, Mount asks UDisks to mount a selected unmounted partition read-only. The OS Polkit agent handles approval/password entry; Raesir never receives the password. NTFS requests `ro`, ext4 `ro,noload`, Btrfs `ro,nologreplay`. There is no writable fallback. Fstab devices are refused because UDisks may ignore caller options. Encrypted storage still needs unlocking through the OS.

Only successful mounts created by Raesir are tracked. Already-mounted partitions are never adopted. Normal app exit first stops its managed Steam client, then unmounts its own partitions in reverse order. Mount point, device, UUID and available kernel mount identity are rechecked to leave removed/replaced mounts alone. No force/lazy unmount is used. If a filesystem is busy or authorization fails, Raesir stays open, reports the problem, and retries cleanup on the next close. Pre-existing mounts remain mounted. Forced termination/system crashes bypass normal cleanup.

UDisks and a desktop authorization agent are required. In Toolbx the explicit mount action can use `/run/host/run/dbus/system_bus_socket` when the ordinary system bus is absent; explicit bus configuration is respected. Host mounts exposed under `/run/host/run/media`, `/run/host/mnt`, `/run/host/media` or `/run/host/var/mnt` are scanned.

On Windows, Linux filesystems need existing OS support such as WinBtrfs or an ext4 reader. Raesir does not install drivers or mount Windows volumes. Mounted readable paths are scanned automatically. WSL cannot attach the disk currently running Windows. Windows storage/runtime behavior still requires hardware validation.

## Data and verification

Inventory/account snapshots are saved atomically in Tauri's app-data directory: `inventory.json`, `owned.json`, `epic-owned.json`. `RAESIR_DATA_DIR` overrides this for diagnostics. Data files are not automatically shared across boots. This prototype reads libraries; launching games remains outside this increment.

```sh
npm run check
npm test
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Readers use fixture manifests/caches, Steam IPC a compiled ABI fixture, Legendary a fake CLI, and mount/unmount lifecycle a fake UDisks service on a private D-Bus. Live account authentication and real mounts/Polkit prompts have not been exercised during agent diagnostics. Windows core code is cross-checked for compilation.

Agent-owned GUI checks must use the user's isolated-gui private display and D-Bus helper, fresh application/XDG data, `RAESIR_DISABLE_HOST_BRIDGE=1`, and `RAESIR_DISABLE_STEAM_IPC=1`. Do not start user clients or request real mounts in diagnostics.
