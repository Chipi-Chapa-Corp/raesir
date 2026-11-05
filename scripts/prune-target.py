#!/usr/bin/env python3
"""Bound development build caches without touching active builds or app data."""
import argparse
from contextlib import contextmanager
import fcntl
import math
import os
from pathlib import Path
import re
import shutil
import time

GIB = 1024**3
GRACE = 300


def profiles(root):
    for parent in [root, *[p for p in root.iterdir() if p.is_dir() and not p.is_symlink() and p.name.count("-") >= 2]]:
        for name in ("debug", "release", "desktop-dev"):
            path = parent / name
            if not path.is_symlink() and (path / ".cargo-lock").is_file():
                yield path


def files(root):
    for directory, dirs, names in os.walk(root, followlinks=False):
        dirs[:] = [d for d in dirs if not (Path(directory) / d).is_symlink()]
        for name in names:
            path = Path(directory) / name
            try:
                if not path.is_symlink():
                    yield path, path.stat()
            except FileNotFoundError:
                pass


def size(root):
    seen = set()
    total = 0
    for _, stat in files(root):
        inode = (stat.st_dev, stat.st_ino)
        if inode not in seen:
            seen.add(inode)
            total += stat.st_blocks * 512
    return total


@contextmanager
def build_lock(profile):
    # Keep this inode in place even when evicting a whole profile, so Cargo
    # cannot acquire a different lock halfway through cleanup.
    with (profile / ".cargo-lock").open("r+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        try:
            yield True
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def compiler_running(workspace):
    # Also guard direct compiler/linker invocations outside Cargo's lock.
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if (proc / "comm").read_text().strip() in {"rustc", "cc", "clang", "ld", "ld.lld", "rust-lld"}:
                if (proc / "cwd").resolve().is_relative_to(workspace):
                    return True
        except (OSError, RuntimeError):
            pass
    return False


def running_binaries(root):
    active = []
    for proc in Path("/proc").iterdir():
        if proc.name.isdigit():
            try:
                exe = (proc / "exe").resolve(strict=True)
                if exe.is_relative_to(root):
                    active.append(exe)
            except (OSError, RuntimeError):
                pass
    return active


def pinned_archives(root):
    pinned = set()
    for path in (root / "dx-session").rglob("link_args.json"):
        try:
            # Inspect only paths; never log the captured command or environment.
            pinned.update(re.findall(r"libdeps-[a-f0-9]+\.a", path.read_text()))
        except OSError:
            pass
    return pinned


def prune(root, *, max_bytes=16 * GIB, dry_run=False, workspace=None):
    root = root.resolve()
    workspace = (workspace or Path(__file__).resolve().parents[1]).resolve()
    if not (root / ".rustc_info.json").is_file():
        return 0
    if root in {Path("/"), Path.home(), workspace}:
        raise ValueError("Refusing to clean outside a Cargo target directory")
    if compiler_running(workspace):
        return 0
    now = time.time()
    pinned = pinned_archives(root)
    removed = 0
    planned = set()
    before_size = size(root)

    def scheduled(path):
        return path in planned or any(parent in planned for parent in path.parents)

    def remove(path):
        nonlocal removed
        if path.is_symlink():
            return
        if scheduled(path):
            return
        try:
            if path.is_dir():
                amount = sum(s.st_blocks * 512 for f, s in files(path) if not scheduled(f))
            else:
                amount = path.stat().st_blocks * 512
            if not dry_run:
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
            removed += amount
            planned.add(path)
        except FileNotFoundError:
            pass

    all_profiles = list(profiles(root))
    for profile in all_profiles:
        with build_lock(profile) as acquired:
            if not acquired:
                continue
            archives = list(profile.glob("libdeps-*.a"))

            def used(archive):
                paired = archive.with_name(archive.name.replace("libdeps-", "rlibs-")).with_suffix(".txt")
                return max(archive.stat().st_mtime, paired.stat().st_mtime if paired.exists() else 0)

            # A full DX link creates a fresh archive whenever workspace rlibs
            # change; DX 0.7.10 never evicts these. Keep two plus the live link.
            archives.sort(key=used, reverse=True)
            for archive in archives[2:]:
                if archive.name not in pinned and now - used(archive) >= GRACE:
                    remove(archive)
                    remove(archive.with_name(archive.name.replace("libdeps-", "rlibs-")).with_suffix(".txt"))
            # save-temps emits pre/post optimization bitcode on every patch.
            # Keep object files, rlibs and incremental data used for later links.
            for bitcode in (profile / "deps").glob("*.bc"):
                if now - bitcode.stat().st_mtime >= GRACE:
                    remove(bitcode)
            variants = {}
            for path in (profile / "incremental").glob("*"):
                if path.is_dir() and not path.is_symlink():
                    name = path.name.rsplit("-", 1)[0]
                    latest = max([path.stat().st_mtime, *[s.st_mtime for _, s in files(path)]])
                    variants.setdefault(name, []).append((latest, path))
            for entries in variants.values():
                for modified, path in sorted(entries, reverse=True)[2:]:
                    if now - modified >= GRACE:
                        remove(path)

    # Bound the overall cache too. Evict inactive build profiles only when the
    # budget is exceeded; the current incremental debug build stays warm.
    total = size(root) - (removed if dry_run else 0)
    active = running_binaries(root)
    evictable = []
    for profile in all_profiles:
        if profile == root / "debug" or any(p.is_relative_to(profile) for p in active):
            continue
        latest = max([profile.stat().st_mtime, *[s.st_mtime for _, s in files(profile)]])
        if now - latest >= GRACE:
            evictable.append((latest, profile))
    for _, profile in sorted(evictable):
        if total <= max_bytes:
            break
        with build_lock(profile) as acquired:
            if acquired and not compiler_running(workspace):
                before = removed
                for path in profile.iterdir():
                    if path.name != ".cargo-lock":
                        remove(path)
                total -= removed - before
    if removed:
        if not dry_run:
            removed = max(0, before_size - size(root))
        verb = "would reclaim" if dry_run else "reclaimed"
        print(f"[dev cache] {verb} {removed / GIB:.2f} GiB", flush=True)
    return removed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=Path, default=Path("target"))
    parser.add_argument("--max-gib", type=float, default=16)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--force", action="store_true", help="Run even if maintenance just finished")
    args = parser.parse_args()
    if not math.isfinite(args.max_gib) or args.max_gib <= 0:
        parser.error("--max-gib must be positive")
    try:
        marker = args.target / ".dev-pruned"
        recent = marker.exists() and time.time() - marker.stat().st_mtime < 60
        if args.dry_run or args.force or not recent:
            prune(args.target, max_bytes=args.max_gib * GIB, dry_run=args.dry_run)
            if not args.dry_run and (args.target / ".rustc_info.json").is_file():
                marker.touch()
    except OSError as error:
        print(f"[dev cache] cleanup deferred: {error}", flush=True)


if __name__ == "__main__":
    main()
