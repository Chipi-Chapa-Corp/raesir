The versioned `mod-recipes.json` catalog is bundled into Raesir at build time.
It is data, with no shell scripts or expressions to execute.

Each game specifies its exact Steam app ID, installer adapter, ModWorkshop game
slug and Nexus browse URL. The first supported installer is `noita`.

A recipe selects exact filenames from a public GitHub release and a Noita mod
folder. The adapter validates `mod.xml` and stages the archive into that folder.
`categories` is optional. `companions` is optional for ordinary mod archives;
when present, its OS entries select an asset, executable and argument array.
The mod and companion always use the same release tag.

An optional `icon` HTTPS URL supplies the card artwork through the shared
thumbnail downloader and disk cache. Entangled Worlds uses the square logo from
its upstream repository. Catalog artwork also applies to existing installations.

Supported argument placeholders are `{game.executable}`, `{profile.config}` and
`{profile.state}`. The latter two point to persistent, separate data for the
selected Raesir profile. Arguments are passed directly to the process, without a
shell. New operations or game installation formats require an adapter change;
adding another recipe using these operations requires only catalog data.

The optional companion `launcher` defaults to `direct`. Entangled Worlds on Linux
uses `steam_proton`: a bundled adapter joins the owning Steam session (including
an existing Flatpak instance), then overrides the proxy's game handoff using the
selected/recorded Proton and its manifest-declared runtime chain. This launcher
is resolved from the current catalog at launch time, so older installations also
receive adapter fixes. It never starts a fresh Steam sandbox or falls back to
an unrelated installed Proton/runtime. A bundled Python 3 bridge preserves
Steam's desktop environment and the proxy's per-lobby networking environment;
downloaded scripts and catalog expressions are not evaluated.
