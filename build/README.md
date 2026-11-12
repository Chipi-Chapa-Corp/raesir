# Distributables

`.github/workflows/build.yml` follows Sele's release workflow: pushes to `main`,
pull requests, and manual runs upload Windows x64 and Linux x64 artifacts. A
`v*` tag also publishes a GitHub release and the Flatpak update repository on
GitHub Pages. The tag, `native/Cargo.toml` version, and latest AppStream release
version must match. Enable **GitHub Actions** as the repository's Pages source
before the first tagged release.

Artifacts are `exposed-games-windows-x64-setup.exe`, `exposed-games-linux-x64.flatpak`, and
`exposed-games.flatpakrepo`. Like Sele, the Flatpak repository is unsigned and uses
HTTPS, and Windows installers are unsigned. No signing secrets are required.

## Linux

Install `flatpak`, `flatpak-builder`, `ostree`, Rust/Cargo, and these Flathub refs:

```sh
flatpak install flathub org.freedesktop.Platform//25.08 \
  org.freedesktop.Sdk//25.08 org.freedesktop.Sdk.Extension.rust-stable//25.08
./scripts/package-flatpak.sh
./scripts/package-flatpak-repository.sh
```

Cargo dependencies are vendored from `Cargo.lock`; compilation runs offline
inside the SDK, using its Rust extension and runtime ABI. CI runs backend tests
first on the validation runner, including their private D-Bus fixtures.
The package includes libmpv with the software render API used by Exposed Games.
AVIF decoding and mod archive extraction use the runtime's dav1d and libarchive.
The manifest grants host filesystem and command access for game clients, with
explicit access to Flatpak clients' data directories. Client command wrappers
forward arguments through `flatpak-spawn --host`.

Build staging, compiler output, and the temporary builder cache are removed on
exit. Final bundles and the Pages repository remain in ignored `dist/`.
Install a downloaded bundle with `flatpak install ./exposed-games-linux-x64.flatpak`.

## Windows

Use an **MSYS2 UCRT64** shell with these packages, matching CI:

```sh
pacman -S --needed git mingw-w64-ucrt-x86_64-{gcc,rust,cmake,ninja,pkgconf,python,dav1d,mpv,libarchive,nsis}
./scripts/package-windows.sh
```

The MSYS2 Rust compiler and DLLs use the same UCRT toolchain. Packaging follows
the executable's DLL imports and the dynamically loaded libmpv and libarchive
dependencies, includes their licenses, and checks library loading and the
executable version before building NSIS. CI also silently installs the result
into a temporary directory and checks the installed executable. Installation is
per user, with Start menu/Desktop shortcuts and an uninstaller. Uninstall keeps
Exposed Games's user data and removes only shipped files from the installation folder.

## Application identity and icon

The application ID is `com.chipichapa.exposedgames` on Linux, including the
Flatpak manifest, desktop entry, AppStream metadata, and native window grouping.
The executable and Cargo package are `exposed-games`.

`build/icon.svg` is the canonical original Exposed Games E-gamepad artwork.
The native window and Flatpak raster icon use `build/icon.png`. Windows executables,
Explorer, taskbar entries, shortcuts, and the installer use `build/icon.ico`,
which includes 16–256 px images. The Windows resource also supplies the application
name and file version. Both raster formats are committed, so release builds do
not need an SVG renderer. After editing the SVG, regenerate them with ImageMagick:

```sh
python3 scripts/generate-icons.py
python3 -m unittest discover -s scripts -p test_branding.py
```

Storage readers retain compatibility with previous application IDs so existing
settings, libraries, and mod profiles remain accessible. Repository and update
URLs use the actual GitHub repository path; changing the application name does
not change that hosted path.
