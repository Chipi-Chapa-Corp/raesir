import fcntl
import importlib.util
import os
from pathlib import Path
import signal
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("prune_target", Path(__file__).with_name("prune-target.py"))
cache = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache)


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "target"
        self.root.mkdir()
        (self.root / ".rustc_info.json").write_text("{}")
        self.compiler = patch.object(cache, "compiler_running", return_value=False)
        self.compiler.start()
        self.addCleanup(self.compiler.stop)
        self.running = patch.object(cache, "running_binaries", return_value=[])
        self.running.start()
        self.addCleanup(self.running.stop)

    def old(self, path, age=3600):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * 8192)
        os.utime(path, (time.time() - age, time.time() - age))
        return path

    def profile(self, name="desktop-dev"):
        path = self.root / name
        path.mkdir(exist_ok=True)
        (path / ".cargo-lock").touch()
        return path

    def test_archive_retention_pins_live_link_and_never_changes_reusable_objects(self):
        profile = self.profile()
        for i in range(5):
            self.old(profile / f"libdeps-{i}.a", 900 + i * 100)
            self.old(profile / f"rlibs-{i}.txt", 900 + i * 100)
        session = self.root / "dx-session/linux/link_args.json"
        session.parent.mkdir(parents=True)
        session.write_text('["libdeps-4.a"]')
        obj = self.old(profile / "deps/current.rcgu.o")
        lib = self.old(profile / "deps/libdependency.rlib")
        bc = self.old(profile / "deps/stale.rcgu.bc")
        fresh = self.old(profile / "deps/building.rcgu.bc", 0)
        cache.prune(self.root)
        self.assertEqual({p.name for p in profile.glob("libdeps-*.a")}, {"libdeps-0.a", "libdeps-1.a", "libdeps-4.a"})
        self.assertFalse(bc.exists())
        self.assertTrue(all(p.exists() for p in (obj, lib, fresh)))

    def test_build_lock_and_running_compiler_prevent_cleanup(self):
        profile = self.profile()
        bc = self.old(profile / "deps/stale.bc")
        with (profile / ".cargo-lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            cache.prune(self.root)
            self.assertTrue(bc.exists())
        with patch.object(cache, "compiler_running", return_value=True):
            cache.prune(self.root)
            self.assertTrue(bc.exists())

    def test_incremental_variants_are_bounded_and_dry_run_is_read_only(self):
        profile = self.profile()
        for i in range(5):
            path = self.old(profile / f"incremental/raesir-{i}/session/work.bin", 900 + i * 100)
            for parent in (path.parent, path.parent.parent):
                os.utime(parent, (time.time() - 900 - i * 100,) * 2)
        before = sorted(str(p) for p in self.root.rglob("*"))
        cache.prune(self.root, dry_run=True)
        self.assertEqual(before, sorted(str(p) for p in self.root.rglob("*")))
        cache.prune(self.root)
        self.assertEqual(len(list((profile / "incremental").iterdir())), 2)

    def test_quota_evicts_legacy_profile_but_keeps_lock_debug_and_running_app(self):
        debug = self.profile("debug")
        self.old(debug / "deps/library.rlib")
        inactive = self.profile("desktop-dev")
        self.old(inactive / "deps/library.rlib")
        active = self.profile("release")
        binary = self.old(active / "raesir")
        for profile in (debug, inactive, active):
            for path, _ in cache.files(profile):
                os.utime(path, (time.time() - 900,) * 2)
            os.utime(profile, (time.time() - 900,) * 2)
        with patch.object(cache, "running_binaries", return_value=[binary]):
            cache.prune(self.root, max_bytes=1)
        self.assertTrue((inactive / ".cargo-lock").exists())
        self.assertFalse((inactive / "deps").exists())
        self.assertTrue(binary.exists())
        self.assertTrue((debug / "deps/library.rlib").exists())

    def test_symlinked_profiles_and_non_target_directories_are_untouched(self):
        external = Path(self.temp.name) / "external"
        external.mkdir()
        (external / ".cargo-lock").touch()
        data = self.old(external / "deps/precious.bc")
        (self.root / "debug").symlink_to(external, target_is_directory=True)
        cache.prune(self.root, max_bytes=1)
        self.assertTrue(data.exists())
        (self.root / ".rustc_info.json").unlink()
        self.assertEqual(cache.prune(self.root), 0)


class DevTests(unittest.TestCase):
    def test_one_run_per_workspace_and_exit_reaps_its_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stub = root / "cargo"
            stub.write_text('''#!/usr/bin/env python3
import os, signal, subprocess, sys, time
from pathlib import Path
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
Path(os.environ["DEV_TEST_PIDS"]).write_text(f"{os.getpid()} {child.pid}")
def stop(*_):
    child.terminate()
    child.wait()
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
while True: time.sleep(0.1)
''')
            stub.chmod(0o755)
            env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                CARGO_TARGET_DIR=str(root / "target"), DEV_TEST_PIDS=str(root / "pids"))
            source = Path(__file__).resolve().parents[1]
            repo = root / "workspace"
            (repo / "scripts").mkdir(parents=True)
            shutil.copy2(source / "dev", repo / "dev")
            shutil.copy2(source / "scripts/prune-target.py", repo / "scripts/prune-target.py")
            process = subprocess.Popen([str(repo / "dev")], cwd=repo, env=env,
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                text=True, start_new_session=True)
            pids = []
            try:
                deadline = time.monotonic() + 5
                while not (root / "pids").exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertTrue((root / "pids").exists())
                pids = [int(p) for p in (root / "pids").read_text().split()]
                duplicate = subprocess.run([str(repo / "dev")], cwd=repo, env=env,
                    stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=5)
                self.assertEqual(duplicate.returncode, 1)
                self.assertIn("already running", duplicate.stderr)
                process.send_signal(signal.SIGTERM)
                output, _ = process.communicate(timeout=10)
                self.assertEqual(process.returncode, 143, output)
                for pid in pids:
                    self.assertFalse(Path(f"/proc/{pid}").exists(), f"orphaned dev process {pid}")
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                if process.stdout:
                    process.stdout.close()
                if pids:
                    try:
                        os.killpg(pids[0], signal.SIGKILL)
                    except ProcessLookupError:
                        pass


if __name__ == "__main__":
    unittest.main()
