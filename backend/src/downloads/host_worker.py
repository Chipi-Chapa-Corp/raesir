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
runtime = request["completion"].get("FerestreRuntime")
ferestre_game = "Ferestre" in request["completion"]
progress = dict(operation=request["operation"], phase="starting", percent=None,
                error=None, downloaded_bytes=None, controllable=runtime is None and not ferestre_game,
                detail="Checking the Xbox license and package…" if ferestre_game else None)

def save():
    with tempfile.NamedTemporaryFile(mode="w", dir=root, delete=False) as output:
        json.dump(progress, output)
        name = output.name
    os.replace(name, root / "progress.json")

def revision():
    try:
        kind, data = next(iter(request["completion"].items()))
        if kind == "FerestreRuntime":
            return None
        if kind == "Legendary":
            return json.loads((Path(data["profile"]) / "installed.json").read_text())[data["app"]].get("version")
        return json.loads(Path(data["record"]).read_text()).get("package_version")
    except (OSError, ValueError, KeyError):
        return None

def installed(before):
    kind, data = next(iter(request["completion"].items()))
    if kind == "FerestreRuntime":
        version = request["version"]
        if version:
            parts = version.split(".")
            if not 3 <= len(parts) <= 5 or not all(p.isascii() and p.isdigit() for p in parts):
                return False
            folders = (Path(data["state"]) / "runtimes" / version,)
        else:
            folders = (Path(data["state"]) / "runtimes").glob("*")
        return any(
            folder.name[:1].isdigit() and (folder / "proton").is_file() and
            (folder / "files/lib/wine/x86_64-windows/xgameruntime.dll").is_file()
            for folder in folders
        )
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

def format_bytes(count):
    for unit, size in (("Gb", 10**9), ("Mb", 10**6), ("KB", 10**3)):
        if count >= size - size // 20_000:
            return f"{count / size:.1f} {unit}"
    return f"{count} B"

def runtime_progress(lines):
    for line in reversed(lines):
        if line.startswith("-- unpacking runtime "):
            return "installing", "Extracting the Xbox runtime…", None
        if line.startswith("-- verified the signature of ferestre-runtime-"):
            return "installing", "Preparing to extract the Xbox runtime…", None
        if line.startswith("-- downloading runtime "):
            fields = line.split()
            if len(fields) < 4:
                continue
            prefix = f".ferestre-runtime-{fields[3]}.tar.xz."
            sizes = []
            for path in Path(runtime["cache"]).glob(".*.part"):
                if path.name.startswith(prefix):
                    try:
                        sizes.append(path.stat().st_size)
                    except OSError:
                        pass
            received = max(sizes, default=0)
            if received:
                label = f"Downloading Xbox runtime · {format_bytes(received)} received"
            else:
                label = "Waiting for the Xbox runtime download…"
            return "downloading", label, received
    return "starting", "Checking the Xbox runtime release…", None

def game_progress(lines):
    for line in reversed(lines):
        if not line.startswith("FERESTRE_PROGRESS "):
            continue
        try:
            event = json.loads(line.split(" ", 1)[1])
            if event.get("schema") != 1:
                continue
            phase = event.get("phase")
            can_pause = event.get("canPause") is True
            if phase == "metadata":
                progress.update(phase="starting", percent=None, downloaded_bytes=None, controllable=can_pause,
                                detail="Checking the Xbox license and package…")
            elif phase == "downloading":
                done = event.get("completedBytes", 0)
                total = event.get("totalBytes")
                if not isinstance(done, int) or done < 0:
                    continue
                if not isinstance(total, int) or total <= 0 or done > total:
                    total = None
                paused = event.get("paused") is True
                network = event.get("networkBytes")
                if type(network) is not int or network < 0:
                    reused = event.get("reusedBytes")
                    if type(reused) is not int or reused < 0:
                        reused = 0
                    network = done - reused if done >= reused else None
                if paused:
                    label = "Paused"
                elif total is not None:
                    label = f"Downloading · {format_bytes(done)} of {format_bytes(total)}"
                else:
                    label = f"Downloading · {format_bytes(done)} received"
                progress.update(phase="paused" if paused else "downloading", detail=label,
                                percent=min(99, done * 100 // total) if total else None,
                                downloaded_bytes=None if paused else network,
                                controllable=can_pause)
            elif phase in ("finalizing", "complete"):
                progress.update(phase="installing", percent=None, downloaded_bytes=None, controllable=False,
                                detail="Finishing the Xbox installation…")
            return True
        except (ValueError, TypeError, AttributeError):
            continue
    return False

def ferestre_failure(lines):
    empty_packages = any("the package list came back empty" in line for line in lines)
    for line in reversed(lines):
        line = line.strip()
        if line == "no user token (entry not found); sign in again":
            return dict(error="XboxSignInRequired")
        if line == "Selection failed" or line.startswith("no PC package found"):
            return dict(detail="Ferestre could not select a compatible PC package for this Xbox edition")
        if line.startswith("No .msixvc file found"):
            if empty_packages:
                return dict(detail="The Xbox Store returned no downloadable PC package. Check again before retrying.")
            return dict(detail="This Xbox game uses a package format that Ferestre cannot install")
        words = line.split()
        if len(words) != 3 or words[0] != "FERESTRE_TRANSFER_DIAGNOSTIC_V1":
            continue
        stage = words[1]
        if stage == "authentication":
            return dict(error="XboxSignInRequired")
        if stage == "license":
            return dict(detail="The Store account could not obtain this Xbox game's license")
        if stage in ("metadata-invalid", "transfer-integrity"):
            return dict(detail="The Xbox game package failed verification")
        if stage in ("metadata-network", "transfer-connect", "transfer-timeout", "transfer-idle",
                     "transfer-range", "transfer-eof", "transfer-status"):
            return dict(detail="The Xbox download connection failed. Retry to continue")
        if stage in ("disk-space", "transfer-io"):
            return dict(detail="Ferestre could not write the game data. Check free space and permissions")
        if stage == "installation-busy":
            return dict(detail="The game is running or its files are in use")
    return None

try:
    game_lease = None
    if "EXPOSED_GAMES_DISABLE_ACTIONS" in os.environ:
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
    if ferestre_game:
        environment.update(XODUS_PROGRESS="json", FERESTRE_GUI_CANCELLATION="1")
    with os.fdopen(os.open(root / "output.log", os.O_CREAT | os.O_TRUNC | os.O_RDWR, 0o600), "w+b") as output:
        before = request.get("previous_version") or revision()
        process = subprocess.Popen([request["program"], *request["args"]], env=environment,
                                   stdin=subprocess.PIPE if ferestre_game else subprocess.DEVNULL,
                                   stdout=output, stderr=output,
                                   start_new_session=True)
        generation = None
        if not ferestre_game and runtime is None:
            progress["phase"] = "downloading"
        failure_reason = None
        while True:
            if os.fstat(output.fileno()).st_size > 8 * 1024 * 1024:
                output.truncate(0)
                output.seek(0)
            try:
                control = json.loads((root / "control.json").read_text())
                if control["generation"] != generation:
                    generation = control["generation"]
                    if ferestre_game:
                        process.stdin.write(b"r" if control["resume"] else b"p")
                        process.stdin.flush()
                    else:
                        os.killpg(process.pid, signal.SIGCONT if control["resume"] else signal.SIGSTOP)
                        progress["phase"] = "downloading" if control["resume"] else "paused"
            except (OSError, ValueError, KeyError):
                pass
            # Read the final log after observing the process exit.
            result = process.poll()
            if progress["phase"] != "paused" or ferestre_game:
                with open(root / "output.log", "rb") as log:
                    log.seek(max(0, os.fstat(log.fileno()).st_size - 65536))
                    lines = log.read().decode("utf-8", errors="replace").splitlines()
                if runtime is not None:
                    progress["phase"], progress["detail"], progress["downloaded_bytes"] = runtime_progress(lines)
                structured = ferestre_game and game_progress(lines)
                if ferestre_game:
                    failure_reason = ferestre_failure(lines) or failure_reason
                for line in reversed(lines) if runtime is None and not structured else ():
                    try:
                        if "Progress:" in line:
                            number = float(line.split("Progress:", 1)[1].split("%", 1)[0].strip())
                            done = None
                        else:
                            counters = json.loads(line)["progress"]
                            done, total = counters["done"], counters["total"]
                            if type(done) is not int or type(total) is not int or not 0 <= done <= total or total == 0:
                                continue
                            number = done * 100 / total
                        if 0 <= number <= 100:
                            progress["percent"] = min(99, int(number))
                            progress["downloaded_bytes"] = done
                            if ferestre_game:
                                progress.update(phase="downloading", detail=None)
                            break
                    except (ValueError, KeyError, TypeError, ZeroDivisionError):
                        pass
            if progress["phase"] != "downloading" or result is not None:
                progress["downloaded_bytes"] = None
            if result is not None:
                extracting = progress["phase"] == "installing"
                try:
                    committed = result == 0 and installed(before)
                except (OSError, ValueError, KeyError, TypeError):
                    if not ferestre_game:
                        raise
                    committed = False
                progress["phase"] = "complete" if committed else "failed"
                progress["percent"] = 100 if progress["phase"] == "complete" else None
                if progress["phase"] == "failed":
                    if runtime is not None:
                        progress["detail"] = ("Could not extract or install the Xbox runtime" if extracting
                                              else "Could not download or verify the Xbox runtime")
                    elif ferestre_game:
                        progress.update(detail=None, controllable=True)
                        if result == 0:
                            progress["detail"] = "Ferestre reported success, but the installed game files or version could not be verified."
                        elif failure_reason:
                            progress.update(failure_reason)
                        else:
                            reason = f"exit code {result}" if result > 0 else f"signal {-result}"
                            progress["detail"] = f"Ferestre stopped ({reason}). See output.log for details."
                    else:
                        progress["detail"] = "The client could not finish this download. Sign in to the client and retry."
                else:
                    progress["detail"] = None
                save()
                break
            save()
            time.sleep(0.4)
except Exception:
    progress.update(phase="failed", detail="The installed client helper could not finish this download.")
    save()
