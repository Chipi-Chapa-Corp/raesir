"""Flatpak host-side worker. Only its own isolated client process group is controlled."""
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1])
lease = open(root / "worker.lock", "a+")
try:
    fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    sys.exit(0)
request = json.loads((root / "request.json").read_text())
progress = dict(operation=request["operation"], phase="starting", percent=None,
                error=None, controllable=True, detail=None)

def save():
    with tempfile.NamedTemporaryFile(mode="w", dir=root, delete=False) as output:
        json.dump(progress, output)
        name = output.name
    os.replace(name, root / "progress.json")

def revision():
    try:
        kind, data = next(iter(request["completion"].items()))
        if kind == "Legendary":
            return json.loads((Path(data["profile"]) / "installed.json").read_text())[data["app"]].get("version")
        return json.loads(Path(data["record"]).read_text()).get("package_version")
    except (OSError, ValueError, KeyError):
        return None

def installed(before):
    kind, data = next(iter(request["completion"].items()))
    if kind == "Legendary":
        records = json.loads((Path(data["profile"]) / "installed.json").read_text())
        record = records[data["app"]]
        if not Path(record["install_path"]).is_dir():
            return False
        actual = record.get("version")
        if not request["version"] or actual == request["version"]:
            return True
        if not actual or before is None or actual == before:
            return False
        try:
            assets = json.loads((Path(data["profile"]) / "assets.json").read_text())
            return any(row.get("app_name", row.get("appName")) == data["app"] and
                       row.get("build_version", row.get("buildVersion")) == actual
                       for row in assets.get(record.get("platform", "Windows"), []))
        except (OSError, ValueError, TypeError):
            return False
    record = json.loads(Path(data["record"]).read_text())
    return (Path(record["dir"]) / ".xodus-streaming.msixvc").is_file() and (not request["version"] or record.get("package_version") == request["version"])

try:
    game_lease = None
    if "RAESIR_DISABLE_ACTIONS" in os.environ:
        raise RuntimeError("actions disabled")
    if request.get("game_lock"):
        path = Path(request["game_lock"])
        path.parent.mkdir(parents=True, exist_ok=True)
        game_lease = open(path, "a+")
        try:
            fcntl.flock(game_lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            progress.update(phase="available", detail="This game is busy · Update later")
            save()
            sys.exit(0)
    game_dir = request.get("game_dir")
    if game_dir:
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                exe = os.readlink(entry / "exe")
                args = (entry / "cmdline").read_bytes().split(b"\0")
                if exe.startswith(game_dir + "/") or any((game_dir + "/").encode() in arg for arg in args):
                    progress.update(phase="available", detail="Close the game to update")
                    save()
                    sys.exit(0)
            except (OSError, PermissionError):
                continue
    environment = os.environ.copy()
    environment.update(request["env"])
    with os.fdopen(os.open(root / "output.log", os.O_CREAT | os.O_TRUNC | os.O_RDWR, 0o600), "w+b") as output:
        before = request.get("previous_version") or revision()
        process = subprocess.Popen([request["program"], *request["args"]], env=environment,
                                   stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                   start_new_session=True)
        generation = None
        progress["phase"] = "downloading"
        while True:
            if os.fstat(output.fileno()).st_size > 8 * 1024 * 1024:
                output.truncate(0)
                output.seek(0)
            try:
                control = json.loads((root / "control.json").read_text())
                if control["generation"] != generation:
                    generation = control["generation"]
                    os.killpg(process.pid, signal.SIGCONT if control["resume"] else signal.SIGSTOP)
                    progress["phase"] = "downloading" if control["resume"] else "paused"
            except (OSError, ValueError, KeyError):
                pass
            if progress["phase"] != "paused":
                with open(root / "output.log", "rb") as log:
                    log.seek(max(0, os.fstat(log.fileno()).st_size - 65536))
                    lines = log.read().decode("utf-8", errors="replace").splitlines()
                for line in reversed(lines):
                    try:
                        if "Progress:" in line:
                            number = float(line.split("Progress:", 1)[1].split("%", 1)[0].strip())
                        else:
                            counters = json.loads(line)["progress"]
                            number = counters["done"] * 100 / counters["total"]
                        if 0 <= number <= 100:
                            progress["percent"] = min(99, int(number))
                            break
                    except (ValueError, KeyError, TypeError, ZeroDivisionError):
                        pass
            result = process.poll()
            if result is not None:
                progress["phase"] = "complete" if result == 0 and installed(before) else "failed"
                progress["percent"] = 100 if progress["phase"] == "complete" else None
                if progress["phase"] == "failed":
                    progress["detail"] = "The client could not finish this download. Sign in to the client and retry."
                save()
                break
            save()
            time.sleep(0.4)
except Exception:
    progress.update(phase="failed", detail="The installed client helper could not finish this download.")
    save()
