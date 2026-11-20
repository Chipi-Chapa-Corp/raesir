"""Read only game executables and send a PID-bound quit request on Linux."""

import json
import os
import signal
import sys


def read_prefix(directory):
    with open(directory + "/environ", "rb") as source:
        for entry in source.read(1024 * 1024).split(b"\0"):
            if entry.startswith(b"WINEPREFIX="):
                return os.fsdecode(entry[len(b"WINEPREFIX="):])
    return None


def wine_executable(directory, launcher):
    if os.path.basename(launcher) not in (
        "wine", "wine64", "wine-preloader", "wine64-preloader",
    ):
        return None
    with open(directory + "/cmdline", "rb") as source:
        command = os.fsdecode(source.read(4096).split(b"\0", 1)[0]).replace("\\", "/")
    if not command.lower().endswith(".exe"):
        return None
    if command[1:3] == ":/":
        drive = command[0].lower()
        if drive == "z":
            command = command[2:]
        else:
            prefix = read_prefix(directory)
            if not prefix:
                return None
            command = os.path.join(prefix, "dosdevices", drive + ":", command[3:])
    elif not os.path.isabs(command):
        command = os.path.join(os.readlink(directory + "/cwd"), command)
    return os.path.realpath(command) if os.path.isfile(command) else None


def inspect(pid):
    directory = f"/proc/{pid}"
    try:
        if os.stat(directory).st_uid != os.getuid():
            return None
        with open(directory + "/stat", "rb") as source:
            stat = source.read().rsplit(b") ", 1)[1].split()
        if stat[0] in (b"Z", b"X"):
            return None
        executable = os.readlink(directory + "/exe")
        if not os.path.isabs(executable):
            return None
        executable = wine_executable(directory, executable) or executable
        return int(stat[19]), os.path.realpath(executable)
    except (OSError, ValueError, IndexError):
        return None


def inside(executable, roots):
    return any(executable.startswith(root + "/") for root in roots)


def run(request):
    targets = [
        (target["key"], tuple(os.path.realpath(root) for root in target["roots"]
                              if os.path.isdir(root) and os.path.realpath(root) != "/"))
        for target in request["targets"]
    ]
    if "quit" in request:
        if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
            raise RuntimeError("This host requires Linux PID descriptors to quit a game safely")
        roots = targets[0][1]
        for process in request["quit"]:
            pid = process["pid"]
            try:
                fd = os.pidfd_open(pid)
            except ProcessLookupError:
                continue
            try:
                observed = inspect(pid)
                if observed and observed[0] == process["started"] and inside(observed[1], roots):
                    signal.pidfd_send_signal(fd, signal.SIGTERM)
            except ProcessLookupError:
                pass
            finally:
                os.close(fd)
        return None
    running = {}
    for row in os.scandir("/proc"):
        if not row.name.isascii() or not row.name.isdecimal():
            continue
        observed = inspect(row.name)
        if observed:
            started, executable = observed
            for key, roots in targets:
                if inside(executable, roots):
                    running.setdefault(key, []).append({"pid": int(row.name), "started": started})
    for processes in running.values():
        processes.sort(key=lambda process: process["pid"])
    return running


try:
    print(json.dumps({"data": run(json.load(sys.stdin))}))
except (OSError, RuntimeError) as error:
    print(json.dumps({"error": str(error)}))
