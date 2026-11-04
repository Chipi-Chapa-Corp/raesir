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

From the project folder, with Rust installed:

```sh
cargo run --release
```

Click a game to see its details. Right-click for game actions. Use **Ctrl + / − / 0** to adjust text size.

On supported Linux Steam builds, Install downloads new games to Steam's main local library and Uninstall removes the local game through your existing Steam session, in the background. Game actions update when the operation finishes. Downloads continue if you close Raesir. The background bridge uses Steam's private client interface in an isolated helper, with compatibility checks; other builds use Steam's own dialogs.

For live UI development, install `dioxus-cli` 0.7.10 once, then run `./dev`. Saved UI changes patch the running window. Press **r** or **R** in the terminal for a full rebuild and restart, including after closing the app. Use this after changes to initialization or state layout.

`./dev` allows one server per workspace and stops its process group on restart or
exit. It automatically maintains `target/` at startup, shutdown, and once a minute:
two recent hot-reload dependency archives and two incremental variants per crate
are retained, and old temporary LLVM bitcode is removed. A 16 GiB cache budget
evicts older inactive build profiles when needed. Active builds, running binaries,
and hot reload's reusable dependencies are preserved, so the budget can temporarily
be exceeded while builds are active. Preview maintenance with
`python3 scripts/prune-target.py --dry-run` or run it immediately with `--force`.

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
