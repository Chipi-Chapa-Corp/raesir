# Raesir

One library for your games across Windows and Linux.

Raesir brings Steam, Epic Games, EA, Ubisoft Connect, Xbox, and Wargaming games together, including libraries managed through Heroic, Lutris, and Bottles.

- Find installed and owned games, with search and filters that remember your choices.
- See games on another OS, including duplicate installations across dual boot.
- Browse game descriptions, trailers, screenshots, reviews, and system requirements.
- Launch, install, and uninstall games through your existing clients, or open their libraries and store pages.

Keep your game clients installed and signed in. Raesir uses their existing libraries and launch tools.

Wargaming support reads Game Center's registered installations on Windows, mounted Windows drives, and configured Wine/Proton prefixes (including Lutris and Bottles). Launch opens the game's page in Game Center; select **Play** there. Install and uninstall actions also use the existing client. Production games can show details and reviews from their matching Steam edition. Complete account libraries, historical playtime, and Wargaming achievements are not imported yet.

## Run

From the project folder, with Rust installed:

```sh
cargo run --release
```

Click a game to see its details. Right-click for game actions. Use **Ctrl + / − / 0** to adjust text size.

On supported Linux Steam builds, Install downloads new games to Steam's main local library and Uninstall removes the local game through your existing Steam session, in the background. Game actions update when the operation finishes. Downloads continue if you close Raesir. The background bridge uses Steam's private client interface in an isolated helper, with compatibility checks; other builds use Steam's own dialogs.

The download queue currently includes three preview entries (one starting at 45%, two queued, with simulated progress) chosen from local Steam library artwork. Preview controls do not affect Steam. Starting or detecting a real Steam operation clears the preview entries. Real Steam downloads use live client progress and per-game stop/resume controls. Set `RAESIR_MOCK_DOWNLOADS=0` to hide preview entries.

For live UI development, install `dioxus-cli` 0.7.10 once, then run `./dev`. Saved UI changes patch the running window. Press **r** or **R** in the terminal for a full rebuild and restart, including after closing the app. Use this after changes to initialization or state layout.

## Games on another OS

Raesir can read libraries on accessible Windows and Linux partitions. On Linux, mount banners offer read-only access with approval through your desktop. Partitions mounted by Raesir are unmounted when you close it.

Windows needs a compatible filesystem reader to access Linux partitions, such as WinBtrfs for Btrfs.

## Xbox games on Linux

Install [Ferestre](https://github.com/icex/ferestre) or [Xodus Nexus](https://github.com/domsvoboda/xodus-nexus), set up your supported games there, and refresh Raesir. Compatibility depends on the game. Raesir shows Xbox's own games rather than duplicating the other stores listed in the Xbox app.

## License

[GNU General Public License v3.0](LICENSE). Bundled third-party assets retain their original licenses.
