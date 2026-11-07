# Raesir

One library for your games across Windows and Linux.

Raesir brings Steam, Epic Games, EA, Ubisoft Connect, Xbox, and Wargaming games together, including libraries managed through Heroic, Lutris, and Bottles.

- Find installed and owned games, with search and filters that remember your choices.
- See games on another OS, including duplicate installations across dual boot.
- Browse game descriptions, trailers, screenshots, reviews, and system requirements.
- Launch, install, and uninstall games through your existing clients, or open their libraries and store pages.

Keep your game clients installed and signed in. Raesir uses their existing libraries and launch tools.

The selector beside the launcher filters switches between All, Purchased (the default),
and Game Pass. Game Pass includes Microsoft's full PC catalog across Game Pass tiers for the US market,
including uninstalled games, and is cached for offline browsing. Refresh updates the
catalog; it also refreshes automatically once a day. Ubisoft games tagged with Microsoft
partner code `3` appear under Game Pass rather than Purchased. Your subscription and
the game's client determine whether you can install it. Purchased excludes demos,
trials, and entries without library membership. The grid renders visible rows, keeps
recent decoded cards in a bounded warm cache, and saves finished card thumbnails to
disk. Twelve current-viewport download slots and eight cached-image slots prioritize
visible cards. Six separate look-ahead slots prefetch in the scroll direction.
Scrolling cancels obsolete HTTP requests; finished thumbnail writes
run separately from first paint. Development builds optimize the image codecs and
card processing so debug builds retain normal thumbnail performance.

Wargaming support reads Game Center's registered installations on Windows, mounted Windows drives, and configured Wine/Proton prefixes (including Lutris and Bottles). Launch opens the game's page in Game Center; select **Play** there. Install and uninstall actions also use the existing client. Production games can show details and reviews from their matching Steam edition. Complete account libraries, historical playtime, and Wargaming achievements are not imported yet.

## Run

Pushes to `main` build a Windows installer and a Linux Flatpak in GitHub Actions.
Download them from the **Build distributables** run's artifacts. Version tags also
publish GitHub releases and the Flatpak update repository, following Sele's
release flow. See [packaging instructions](build/README.md) for local builds.

From the project folder, with Rust installed:

```sh
cargo run --release
```

Click a game to see its details. Right-click for game actions. Use **Ctrl + / − / 0** to adjust text size.

On supported Linux Steam builds, Install downloads new games to Steam's main local library and Uninstall removes the local game through your existing Steam session, in the background. Game actions update when the operation finishes. Downloads continue if you close Raesir. The background bridge uses Steam's private client interface in an isolated helper, with compatibility checks; other builds use Steam's own dialogs.

Steam ownership is read from the installed, signed-in client on 64-bit Windows
and Linux, without a Web API key. Failed or signed-out lookups keep the prior
library. Steam's native background download controls currently require a
compatible Linux client; Windows uses Steam's dialogs and reads committed
installation manifests for progress.

Settings opens a popup with **Automatic game updates**, enabled by default.
Available updates replace Play with **Update** on cards and game pages. With
automatic updates enabled, Raesir starts those updates; turning it off leaves
them available for manual installation. Running transfers show **Pause** and
their progress, and paused transfers show **Resume**. The Downloads popup uses
the same state and controls. This setting governs requests made by Raesir;
existing clients retain their own automatic-update settings.

Update checks run at startup and periodically while Raesir is open. Steam uses
its client's known update state; Epic checks installed Legendary/Heroic profiles,
and Xbox on Linux can use an installed Ferestre helper. Epic and Ferestre
transfers use those clients' existing accounts and install locations. Managed
workers retain progress across Raesir restarts, continue after Raesir closes,
and coordinate with mod/game leases. A transfer is complete only after the
client exits successfully and commits its expected installation version.

Clients without verified direct controls continue to handle their own install
dialogs. Raesir tracks those requested installs in Downloads until their
installation metadata confirms completion, and offers **Open in game client**.
It does not invent percentages or expose a nonfunctional Pause button for them.

For development, run `./dev`. It builds incrementally with optimized UI and image
processing, then launches Raesir. Close the app or press **Ctrl+C**, then run
`./dev` again to pick up changes. No watcher or Dioxus CLI is needed.

`./dev` allows one run per workspace and stops its process group on exit. It
maintains `target/` at startup and shutdown, retaining two incremental variants
per crate and removing old temporary LLVM bitcode and excess legacy hot-patch
archives. A 16 GiB cache budget evicts older inactive build profiles when needed.
Active builds, running binaries, and the current debug profile are preserved, so
the budget can temporarily be exceeded. Preview maintenance with
`python3 scripts/prune-target.py --dry-run` or run it immediately with `--force`.

## Mods

Supported games have a **Mods** bubble on their details page. Browse a virtualized
card grid with cached mod icons. The header switches to mod search, profile selection,
and All / Installed / Not installed filters. Click a card to open its details inside
Raesir and install it with its dependencies. This uses public, key-free Thunderstore APIs and
[thunderstore-engine](https://github.com/endoze/thunderstore-engine); games match the
upstream store identifiers rather than title guesses.

Noita also has ModWorkshop browsing, Nexus browser-download imports, and bundled
recipes from [`backend/data/mod-recipes.json`](backend/data/mod-recipes.json).
Entangled Worlds is the first recipe. It downloads the mod and the native proxy
from the same public GitHub release; arbitrary GitHub URLs are not accepted.
The **Install Mod** card imports an archive downloaded from a Noita Nexus page.
There is no general local-folder import, and no API keys are required.

Installed mods are checked automatically at startup and hourly while Raesir is
open, across all Raesir profiles, including inactive profiles and games currently
uninstalled. Thunderstore, ModWorkshop and bundled recipes update in the
background. ModWorkshop retains the chosen file variant; old installs with an
ambiguous variant ask you to choose their file once. Nexus imports continue to
require a browser download and re-import, and are reported as manual updates.

Updates preserve disabled mods, dependency install reasons, profile selection,
and companion settings. Downloads do not block Play. A running game or proxy
defers its update until it closes, with a retry each minute. Failed downloads
retry after fifteen minutes and keep the current revision. Changes made to a
profile while an update downloads also defer publication. Update results appear
in notifications and the mod browser.

Noita archives are validated and installed into profile revisions. ZIP is built
in; 7z/RAR use the system `libarchive` library. Multiple ModWorkshop files get a
file-selection popup. Ordinary modded launches use Steam; an Entangled Worlds
launch opens its proxy, where you host/join a lobby and start Noita. Game mod
folders and activation settings are staged for the session and restored on exit
or recovered after an interruption. Existing manual mod-folder conflicts are
reported before launch. Profiles manage mods; they do not create separate Noita
world saves.

Noita's normal **Play** explicitly disables all entries in its mod configuration,
including manually enabled Entangled Worlds and Workshop mods. **Launch modded**
enables the selected profile for that session and restores the prior disabled
state on exit. Installed mods and world saves are kept. Close a running modded
game/proxy before switching modes.

On Linux, Entangled Worlds joins the Steam session that owns Noita. Flatpak Steam
uses `flatpak enter` on an existing instance; Raesir checks the owning process,
PID namespace and file access before opening the proxy. It preserves the proxy's
lobby environment and launches Noita with the recorded Proton tool and the Steam
runtimes declared by that tool's installed manifests. Missing sessions, runtimes
or sandbox access report an error and restore staged files. Launch Noita once in
Steam before using this integration. The Linux Steam bridge requires Python 3 (provided
by Flatpak Steam's runtime).

Companion settings and multiplayer state stay with the selected profile in
Steam-accessible storage, including across mod updates. Older settings migrate
without overwriting an existing destination. Existing recipe installations use
the current launcher adapter without reinstalling their downloads.

Select mod profiles from the profile menu, with pencil/trash controls on each row.
The adjacent plus pill creates a profile. Each profile has its own mod set.
Mod cards keep square icons and open a details page inside Raesir, using the same
layout as game pages. **Launch modded** appears beside Play
on the details page and library cards when a local installation has enabled mods.
Steam’s normal Play explicitly disables the loader. Installs publish a complete profile
revision only after all dependencies succeed, and retain the previous revision.
Raesir records direct installs separately from automatic dependencies in each
profile revision. Uninstall confirmation lists automatic dependencies that are
no longer needed and removes them together. Shared dependencies, dependencies
needed by disabled mods, and direct installs stay installed. Older installs with
no recorded reason are kept rather than guessed to be automatic.

On Linux, modded Steam launches use the running Steam client's runtime. Temporary
launch options preserve existing settings and are restored after exit; loader files
staged beside the game are backed up and restored, with recovery on the next scan
after an interruption. Steam client interfaces have compatibility checks;
unsupported builds leave the game unlaunched. Flatpak Steam profiles stay within Steam's accessible
data directory without expanding its sandbox permissions.

## Cover artwork

Raesir uses official store artwork first. For Steam games with missing portraits or
Steam's generated banner with blurred padding, it automatically checks
[PCGamingWiki](https://www.pcgamingwiki.com/wiki/PCGamingWiki:API) by exact Steam app ID.
If that has no suitable portrait, it checks public [LaunchBox Games Database](https://gamesdb.launchbox-app.com/)
pages and verifies the exact Steam app ID before selecting a front cover.
This uses public, key-free lookups: no account, API key, or configuration is needed.
Designed official portraits and explicit custom covers keep priority. Wide banners
from the fallback source are rejected.

The fallback uses card-sized thumbnails, the existing bounded, cancellable artwork
workers, and a rate limit for wiki lookups. Downloaded portraits and finished card
thumbnails are cached for offline use. Failed lookups are cooled down and keep the
existing official artwork. Existing Steam thumbnails refresh automatically, with
no manual cache deletion.

## Games on another OS

Raesir can read libraries on accessible Windows and Linux partitions. On Linux, mount banners offer read-only access with approval through your desktop. Partitions mounted by Raesir are unmounted when you close it.

Windows needs a compatible filesystem reader to access Linux partitions, such as WinBtrfs for Btrfs.

## Xbox games on Linux

Install [Ferestre](https://github.com/icex/ferestre) or [Xodus Nexus](https://github.com/domsvoboda/xodus-nexus), set up your supported games there, and refresh Raesir. Compatibility depends on the game. Raesir shows Xbox's own games rather than duplicating the other stores listed in the Xbox app.

## License

[GNU General Public License v3.0](LICENSE). Bundled third-party assets retain their original licenses.
