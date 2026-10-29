# Raesir

Native GPUI game library for Windows and Linux, using the existing Rust discovery and client backend.

```sh
cargo run --release
# F12 frame timing overlay:
cargo run --release --features profile
```

Right-click a card for launch, store and uninstall options. The unchanged prototype remains in `gpui-mock`; the previous Tauri frontend is available with `npm run tauri dev`.

## Library and clients

| Store | Sources |
| --- | --- |
| Steam | Native/Flatpak manifests, library folders; cached ownership and experimental Linux client IPC |
| Epic | Classic launcher manifests, native Linux/Windows Preview `installations.jsonl`, Legendary and Heroic caches |
| EA | EA app/Origin installer XML, configured library folders and Windows registry; Lutris account cache |
| Ubisoft | Connect installation registry, cached protobuf/YAML product configurations and Lutris |
| Xbox | Microsoft GDK packages, registered Windows packages and Xbox Live UWP manifests; Ferestre and Xodus Nexus state |

Known native, Wine, Steam Proton, Heroic, Lutris and Bottles locations are scanned automatically, including readable mounted Windows/Linux installations. Store IDs stay separate. Missing/unreadable sources retain historical installations as unavailable; verified removal clears them. Discovery is bounded to known metadata locations rather than crawling game payloads.

Installed means a ready installation on **either OS**. Other OS adds a constraint, so it always narrows results. Gray cards indicate that a game is not installed on the current OS; OS badges identify foreign installations. Store filters combine with OR, while search/status/location combine with AND.

Launches use the registered client, configured third-party launcher or prefix runner. Existing client sessions remain under their owner's control. Uninstall uses vendor tooling: where no documented per-game removal API exists, Raesir opens the client's management UI and explains the remaining step. Store actions use product pages when identifiers are available, otherwise the store's title search. Thumbnails are downloaded lazily with bounded concurrency, pooled connections, disk caching and a negative cache. Cached inventory appears before filesystem and Steam ownership refreshes.

Uninstalled account titles require an available account cache: Steam IPC, Heroic/Legendary, EA/Lutris, Ubisoft's local cache or Ferestre. Raesir does not decrypt proprietary official-client account caches or claim that installation records establish ownership. Refresh Epic uses an installed Legendary CLI with its existing login (`RAESIR_LEGENDARY_BINARY` can override PATH); it does not import official launcher credentials. Windows Steam ownership IPC is not implemented.

## Xbox on Linux

[Ferestre](https://github.com/icex/ferestre) and [Xodus Nexus](https://github.com/domsvoboda/xodus-nexus) provide the recent GDK/MSIXVC runtime work. Set up supported games there, then refresh Raesir. Their launch paths retain responsibility for Microsoft login, licensing and the patched runtime. Compatibility is title-dependent; ordinary UWP/Appx packages remain Windows-only. Raesir reads Microsoft package metadata, not the Xbox app's aggregated Steam/EA/Epic listings. Protected WindowsApps content needs existing read access; Raesir does not change permissions. Detection is limited to packages exposing supported game metadata.

## Mounts and lifecycle

Linux mount banners request **read-only** UDisks mounts with OS-managed Polkit approval (`ro`; ext4 adds `noload`, Btrfs adds `nologreplay`). No passwords enter Raesir. Fstab-managed devices are refused; encrypted volumes must first be unlocked through the OS. Normal close stops only Steam sessions started by Raesir and unmounts only partitions it mounted, in reverse order with identity checks. Busy mounts keep the app open for retry. Existing mounts and Steam sessions are not adopted. Crashes/forced termination can bypass cleanup.

Windows needs existing filesystem support (for example WinBtrfs or an ext4 reader). Raesir reports missing support and scans readable mounts; it does not install drivers. Runtime behavior on Windows and real client/game launches still need hardware validation.

## Development checks

```sh
cargo test --release
npm run check
npm test
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Fixtures cover readers, source retention, ownership IPC, Legendary subprocesses and mount cleanup on a private D-Bus. Agent GUI tests use the isolated-gui helper, separate app/XDG data, and `RAESIR_DISABLE_STEAM_IPC=1`, `RAESIR_DISABLE_HOST_BRIDGE=1`, `RAESIR_DISABLE_ACTIONS=1`. `RAESIR_DATA_DIR` redirects inventory snapshots. No live authentication, game removal or real mounting is performed in diagnostics. Store logos are CC0 Simple Icons SVGs.
