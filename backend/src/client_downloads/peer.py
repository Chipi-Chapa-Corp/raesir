"""Flatpak host process check. No GUI, credentials, HTTP or control requests."""
import os
from pathlib import Path
import sys

port, store, root, prefix = sys.argv[1:]
port = int(port)
assert port in (4695, 9394, 9222) and store in ("ea", "ubisoft")
root, prefix = Path(root).resolve(), Path(prefix).resolve()
expected = "eadesktop.exe" if store == "ea" else "upc.exe"
def same_directory(left, right):
    try:
        return left.is_dir() and right.is_dir() and left.samefile(right)
    except OSError:
        return False

inodes = set()
has_client = False
for process in Path("/proc").iterdir():
    if not process.name.isdigit():
        continue
    try:
        if process.stat().st_uid != os.getuid():
            continue
        comm = (process / "comm").read_text().strip().lower()
        if comm != expected and not comm.startswith("wineserver"):
            continue
        environment = dict(part.split(b"=", 1) for part in (process / "environ").read_bytes().split(b"\0") if b"=" in part)
        selected = Path(os.fsdecode(environment.get(b"WINEPREFIX", b"")))
        if not selected.is_absolute() or not (same_directory(selected, prefix) or same_directory(process / "root" / str(selected).lstrip("/"), prefix)):
            continue
        if comm == expected:
            argument = os.fsdecode((process / "cmdline").read_bytes().split(b"\0")[0]).replace("\\", "/")
            if len(argument) > 2 and argument[1] == ":":
                path = (prefix / "dosdevices" / argument[:2].lower()).resolve() / argument[2:].lstrip("/")
            else:
                path = Path(argument)
            if not (same_directory(path.parent, root) or same_directory(process / "root" / str(path.parent).lstrip("/"), root)):
                continue
            has_client = True
        for fd in (process / "fd").iterdir():
            try:
                link = os.readlink(fd)
                if link.startswith("socket:["):
                    inodes.add(link[8:-1])
            except OSError:
                pass
    except (OSError, ValueError):
        continue
verified = False
if has_client:
    for table in ("/proc/net/tcp", "/proc/net/tcp6"):
        for line in Path(table).read_text().splitlines()[1:]:
            fields = line.split()
            if len(fields) > 9 and fields[3] == "0A" and fields[9] in inodes and int(fields[1].rsplit(":", 1)[1], 16) == port:
                verified = True
print("verified" if verified else "unavailable")
