#!/usr/bin/env python3
"""Stage the application and the transitive DLLs of its linked/dlopen libraries."""
import ctypes
import os
import pathlib
import re
import shutil
import subprocess
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
os.chdir(root)
prefix = pathlib.Path(shutil.which("gcc")).resolve().parent.parent
stage = root / "dist/windows"
if stage.exists():
    shutil.rmtree(stage)
stage.mkdir(parents=True)
target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", root / "target"))
shutil.copyfile(target / "release/raesir.exe", stage / "raesir.exe")
shutil.copyfile(root / "LICENSE", stage / "LICENSE")

dlls = {path.name.lower(): path for path in (prefix / "bin").glob("*.dll")}
def library(*names):
    for name in names:
        if name in dlls:
            return dlls[name]
    raise RuntimeError(f"Required library is missing: {names}")

mpv = library("libmpv-2.dll", "mpv-2.dll")
archive = library("archive.dll", "libarchive-13.dll")
pending = [stage / "raesir.exe", mpv, archive]
copied = set()
packages = set()
while pending:
    path = pending.pop()
    if path.name.lower() in copied:
        continue
    copied.add(path.name.lower())
    if path.parent != stage:
        shutil.copyfile(path, stage / path.name)
        unix_path = subprocess.check_output(["cygpath", "-u", str(path)], text=True).strip()
        packages.add(subprocess.check_output(["pacman", "-Qqo", unix_path], text=True).strip())
    output = subprocess.check_output(["objdump", "-p", str(path)], text=True)
    for name in re.findall(r"DLL Name:\s*(\S+)", output):
        if name.lower() in dlls:
            pending.append(dlls[name.lower()])
        elif not name.lower().startswith(("api-ms-", "ext-ms-")) and not (
            pathlib.Path(os.environ["SYSTEMROOT"]) / "System32" / name
        ).is_file():
            raise RuntimeError(f"Unresolved dependency of {path.name}: {name}")

# The mod archive loader uses this platform-independent Windows filename.
if archive.name.lower() != "archive.dll":
    shutil.copyfile(stage / archive.name, stage / "archive.dll")
licenses = stage / "third-party"
for package in sorted(packages):
    listing = subprocess.check_output(["pacman", "-Ql", package], text=True)
    for line in listing.splitlines():
        unix_path = line.split(" ", 1)[1]
        if "/share/licenses/" not in unix_path or unix_path.endswith("/"):
            continue
        path = pathlib.Path(subprocess.check_output(["cygpath", "-w", unix_path], text=True).strip())
        output = licenses / "runtime" / unix_path.split("/share/licenses/", 1)[1]
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, output)
subprocess.run(["python", "scripts/collect-licenses.py", str(licenses)], check=True)

# Check both optional dlopen dependencies, and start the executable without a GUI.
with os.add_dll_directory(str(stage)):
    ctypes.CDLL(str(stage / mpv.name))
    ctypes.CDLL(str(stage / "archive.dll"))
version = tomllib.loads((root / "native/Cargo.toml").read_text())["package"]["version"]
output = subprocess.check_output([str(stage / "raesir.exe"), "--version"], text=True).strip()
if output != f"Raesir {version}":
    raise RuntimeError(f"Unexpected executable version: {output!r}")

# Delete only the files shipped by this installer, retaining user-added files.
commands = []
for path in sorted(stage.rglob("*")):
    if path.is_file():
        relative = str(path.relative_to(stage)).replace("/", "\\")
        commands.append(f'Delete "$INSTDIR\\{relative}"')
for path in sorted((path for path in stage.rglob("*") if path.is_dir()), key=lambda p: len(p.parts), reverse=True):
    relative = str(path.relative_to(stage)).replace("/", "\\")
    commands.append(f'RMDir "$INSTDIR\\{relative}"')
(root / "dist/windows-uninstall.nsh").write_text("\n".join(commands) + "\n", encoding="utf-8")
