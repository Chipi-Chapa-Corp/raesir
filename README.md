# Raesir

<img src="build/icon.svg" width="96" height="96" alt="Raesir icon">

Existing settings, cached libraries, and mod profiles from earlier releases
are reused from their original directories when no Raesir storage exists.
New installations use `com.chipichapa.raesir`; the executable is
`raesir` and diagnostic environment variables use `RAESIR_`.

One library for your games across Windows and Linux.

Raesir brings Steam, Epic Games, EA, Ubisoft Connect, Xbox, and Wargaming games together, including libraries managed through Heroic, Lutris, and Bottles.

- Find installed and owned games, with search and filters that remember your choices.
- See games on another OS, including duplicate installations across dual boot.
- Browse game descriptions, trailers, screenshots, reviews, and system requirements.
- Launch, install, and uninstall games through your existing clients, or open their libraries and store pages.

Keep your game clients installed and signed in. Raesir uses their existing libraries and launch tools.

On Linux, Steam cards show the overall tier from [ProtonDB](https://www.protondb.com/)
community reports. Hover over the badge to see the tier. Raesir caches these
ratings for offline use. ProtonDB publishes its reports under the [Open Database
License](https://github.com/bdefore/protondb-data).

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

Wargaming support reads Game Center's registered installations on Windows, mounted Windows drives, and configured Wine/Proton prefixes (including Lutris and Bottles). The client badge on the game's page opens the game in Game Center for play and installation management. Production games can show details and reviews from their matching Steam edition. Complete account libraries, historical playtime, and Wargaming achievements are not imported yet.

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

When you select **Play**, the button shows **Starting…** until Raesir
detects the game process. The pause icon then offers **Quit game**. Play returns
after the game exits. Linux game process tracking uses Python 3 on the host.

Steam reviews default to the store’s Summary view: most helpful reviews from the
past 30 days alongside recently posted reviews, in English and across all purchase
types. Review type, purchase type, language, date range, playtime at review, and
display controls query Steam directly. Date and playtime filters support custom
ranges, and each list can load additional pages. Reviewer names link to their
public Steam profiles.

On compatible 64-bit Windows and Linux Steam builds, Install downloads new games to Steam's main local library and Uninstall removes the local game through your existing Steam session, in the background. Game actions update when the operation finishes. Downloads continue if you close Raesir. The background bridge uses Steam's private client interface in an isolated helper, with compatibility checks; unsupported Steam builds leave these controls unavailable.

Steam ownership is read from a running, signed-in client on 64-bit Windows and
Linux, without a Web API key. Failed or signed-out lookups keep the prior
library. Steam progress comes from the running client, and the installation
manifest confirms completion. Library refresh and update checks leave a closed
Steam client closed.

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

EA app and Ubisoft Connect expose their native download queues to Raesir on
Windows and in discovered Linux Wine/Proton profiles, including configured
Lutris and Bottles runners. Connected clients support updates and Pause/Resume
in Raesir. Ubisoft's pause control belongs to its active queue, so queued cards
cannot pause another game.

Startup, library refresh, and automatic update checks leave closed desktop
clients closed. Opening EA or Ubisoft from the client badge on a game's page
enables the download interface for a matching local profile. If your client
is already open without the interface, quit it once and use that badge again. Raesir verifies the owning executable
and prefix before connecting and leaves existing sessions running.

Steam, Legendary, and Ferestre support background installation when a
compatible local profile is available. Raesir offers installation and removal
controls when it can send those operations directly. Use the client badge on
a game's page for operations that need the client's interface. When starting a game
through Steam or Heroic, Raesir requests the client's background launch mode.
A client can still present its own sign-in or DRM prompts.

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

Open **Mods** from a PC game's details page to browse and search the available
catalogs together. Results include their source. Raesir checks Nexus Mods,
GameBanana, ModWorkshop, Steam Workshop for Steam games, and Thunderstore where
its registry lists the game. Open a card for details and a link to the source.

For **Deadlock**, Raesir installs supported GameBanana VPK packages into the
selected profile. Use **Import VPK or archive** for local packages. **Launch modded**
loads the profile and restores the game's original configuration when Deadlock
closes. **Dota 2** and **Counter-Strike 2**
show public Steam Workshop listings alongside GameBanana. Source 2 mods target
the specific game they were made for.

On a mod's details page, **Open game folder** helps with author instructions
that refer to files in the local installation.

Public search and mod details work while signed out. In a registered build, **Sign in to
Nexus Mods** opens browser approval and the banner disappears when Raesir is
connected. **Settings** lets you disconnect the account. Builds awaiting
registration show that status for games with Nexus installation support.

Signed-in accounts can install mods where Raesir has an installer. Raesir uses
[Modde](https://github.com/caniko/modde-rs)'s game registry to identify supported
store editions and its installer to analyze and stage archives. Mods for a
supported local installation are copied into the game using its layout. Raesir
tracks the files for uninstall and reports any collision with existing files.
Noita retains its profile installer. Archives
requiring an interactive installer or a separate deployment step are reported
when opened. Check each mod's requirements before playing.

Raesir downloads the main file when Nexus permits a direct download. If several
files are suitable, Raesir asks which one to use. When Nexus requires a website
download, choose **Import archive** with the downloaded file.

Games with managed mod profiles have profile selection and installation filters
in the Mods header. Thunderstore mods can be installed with their dependencies
through [thunderstore-engine](https://github.com/endoze/thunderstore-engine), which
matches the game by its store identifier.

Noita also has ModWorkshop browsing, Nexus browser-download imports, and bundled
recipes from [`backend/data/mod-recipes.json`](backend/data/mod-recipes.json).
Entangled Worlds is the first recipe. It downloads the mod and the native proxy
from the same public GitHub release; arbitrary GitHub URLs are not accepted.
The **Install Mod** card imports an archive downloaded from a Noita Nexus page.

On Steam, Stardew Valley Nexus mods can be installed through Modde from **Mods**.
Set up [SMAPI](https://smapi.io/) to load them. Existing Raesir SMAPI profiles
are still accessible from **Mods**. On native Steam, **Launch modded** can
continue using those profiles. On Xbox PC, the Mods view supports browsing
and **Show profile mods** can reveal files from an existing profile.

Raesir checks Thunderstore, ModWorkshop, and bundled recipes for updates at
startup and hourly while open, including inactive profiles and uninstalled games. ModWorkshop retains the chosen file variant; old installs with an
ambiguous variant ask you to choose their file once. Nexus imports continue to
require a browser download and re-import, and are reported as manual updates.

Updates preserve disabled mods, dependency install reasons, profile selection,
and companion settings. Mods can be installed, removed, and updated while a game
or proxy is running. Each session keeps its original profile revision; changes
take effect on the next launch. Downloads do not block Play. Failed downloads
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

On Linux, discovery treats multi-device Btrfs filesystems as one pool and scans the mount points reported across its members. An unmounted pool's banner shows its device count and raw capacity.

Windows needs a compatible filesystem reader to access Linux partitions, such as WinBtrfs for Btrfs.

## Xbox games on Linux

On x86-64 Linux, open the Xbox library and select **Set up Xbox** if no Xbox client is installed.
Raesir verifies the Ferestre release signature and installs the CLI and its supporting files in your user data directory.
Select **Install Xbox runtime** to download the runtime before or after signing in.
The Xbox library and Downloads show the current stage and bytes received. The background download continues if you close Raesir.
Select **Sign in to Xbox** to open the Microsoft sign-in window and import the account's library. Game downloads and launches use the controls in Raesir.

Raesir also detects an existing [Ferestre](https://github.com/icex/ferestre) or
[Xodus Nexus](https://github.com/domsvoboda/xodus-nexus) installation.
Compatibility depends on the game. The Xbox library shows Xbox PC titles, with games managed by other stores listed under those stores.

## License

[GNU General Public License v3.0](LICENSE). Bundled third-party assets retain their original licenses.
