# Raesir

One library for your games across Windows and Linux.

Raesir brings Steam, Epic Games, EA, Ubisoft Connect, and Xbox games together, including libraries managed through Heroic, Lutris, and Bottles.

- Find installed and owned games, with search and filters that remember your choices.
- See games on another OS, including duplicate installations across dual boot.
- Browse game descriptions, trailers, screenshots, reviews, and system requirements.
- Launch games through your existing clients, open their libraries and store pages, or uninstall games.

Keep your game clients installed and signed in. Raesir uses their existing libraries and launch tools.

## Run

From the project folder, with Rust installed:

```sh
cargo run --release
```

Click a game to see its details. Right-click for game actions. Use **Ctrl + / − / 0** to adjust text size.

For live UI development, install `dioxus-cli` 0.7.10 once, then run `./dev`. Saved UI changes patch the running window. Changes to initialization or state layout need a restart.

## Games on another OS

Raesir can read libraries on accessible Windows and Linux partitions. On Linux, mount banners offer read-only access with approval through your desktop. Partitions mounted by Raesir are unmounted when you close it.

Windows needs a compatible filesystem reader to access Linux partitions, such as WinBtrfs for Btrfs.

## Xbox games on Linux

Install [Ferestre](https://github.com/icex/ferestre) or [Xodus Nexus](https://github.com/domsvoboda/xodus-nexus), set up your supported games there, and refresh Raesir. Compatibility depends on the game. Raesir shows Xbox's own games rather than duplicating the other stores listed in the Xbox app.

## License

[GNU General Public License v3.0](LICENSE). Bundled third-party assets retain their original licenses.
