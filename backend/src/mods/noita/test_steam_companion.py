"""Fixture-only tests: never enter Steam or start a real companion/game."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('steam_companion', HERE / 'steam_companion.py')
bridge = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bridge)


class SteamCompanionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='steam companion ')
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)

    def test_session_matches_owning_steam_and_rejects_another_client_or_stale_pid(self):
        root = self.base / 'selected Steam'
        executable = root / 'ubuntu12_32/steam'
        executable.parent.mkdir(parents=True)
        executable.touch()
        proc = self.base / 'proc'
        wrong = proc / '100'
        wrong.mkdir(parents=True)
        (wrong / 'comm').write_text('steam\n')
        other = self.base / 'other Steam/steam'
        other.parent.mkdir()
        other.touch()
        (wrong / 'exe').symlink_to(other)
        selected = proc / '200'
        selected.mkdir()
        (selected / 'comm').write_text('steam\n')
        (selected / 'exe').symlink_to(executable)
        self.assertEqual(bridge.steam_process(root, proc), selected)
        (selected / 'exe').unlink()
        with self.assertRaisesRegex(RuntimeError, 'No running Steam'):
            bridge.steam_process(root, proc)

    def test_restore_steam_session_environment_removes_loader_pollution_without_logging(self):
        process = self.base / 'process'
        process.mkdir()
        (process / 'environ').write_bytes(
            b'DISPLAY=:steam\0DBUS_SESSION_BUS_ADDRESS=steam-bus\0'
            b'LD_PRELOAD=scout.so\0LD_LIBRARY_PATH=scout\0LD_AUDIT=audit.so\0'
            b'STEAM_RUNTIME=scout\0STEAM_RUNTIME_LIBRARY_PATH=scout\0'
            b'PATH=/fixture/bin\0LANG=en_US.UTF-8\0')
        env = bridge.steam_environment(process)
        self.assertEqual(env['DISPLAY'], ':steam')
        self.assertEqual(env['DBUS_SESSION_BUS_ADDRESS'], 'steam-bus')
        self.assertEqual(env['SteamAppId'], '881100')
        for key in ('LD_PRELOAD', 'LD_LIBRARY_PATH', 'LD_AUDIT', 'STEAM_RUNTIME', 'STEAM_RUNTIME_LIBRARY_PATH'):
            self.assertNotIn(key, env)

    def test_game_handoff_keeps_lobby_address_argv_and_required_runtime_order(self):
        output = self.base / 'received.json'
        game = self.base / 'fake game'
        game.write_text('#!/usr/bin/python3\nimport json,os,sys\nfrom pathlib import Path\n'
                        'Path(os.environ["TEST_OUTPUT"]).write_text(json.dumps({"args":sys.argv[1:],"cwd":os.getcwd(),"env":dict(os.environ)}))\n')
        game.chmod(0o700)
        plan = {
            'root': str(self.base / 'Steam'), 'game_dir': str(self.base),
            'compat_data': str(self.base / 'compatdata/881100'),
            'libraries': ['/library one/steamapps', '/library two/steamapps'],
            'tools': ['/Proton chosen', '/Runtime chosen'],
            'game_command': [str(game), 'a spaced argument', '$(literal)'],
        }
        file = self.base / 'plan.json'
        file.write_text(json.dumps(plan))
        env = dict(os.environ, NP_NOITA_ADDR='127.0.0.1:54321', TEST_OUTPUT=str(output))
        result = subprocess.run(['python3', str(HERE / 'steam_companion.py'), 'game', str(file), 'extra'],
                                env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        got = json.loads(output.read_text())
        self.assertEqual(got['args'], ['a spaced argument', '$(literal)', 'extra'])
        self.assertEqual(got['env']['NP_NOITA_ADDR'], '127.0.0.1:54321')
        self.assertEqual(got['env']['STEAM_COMPAT_DATA_PATH'], plan['compat_data'])
        self.assertEqual(got['env']['STEAM_COMPAT_TOOL_PATHS'], '/Proton chosen:/Runtime chosen')
        self.assertEqual(got['env']['STEAM_COMPAT_LIBRARY_PATHS'], '/library one/steamapps:/library two/steamapps')

    def test_inaccessible_payload_fails_preflight_before_a_proxy_can_start(self):
        plan = dict(root=str(self.base), proxy=str(self.base / 'missing proxy'),
                    game_dir=str(self.base), game_command=['missing runtime'], tools=[], runtime=str(self.base))
        with patch.object(bridge, 'steam_process', return_value=self.base), patch.object(bridge.Path, 'readlink', return_value=Path('same-pid-namespace')):
            with self.assertRaisesRegex(RuntimeError, 'Steam cannot access'):
                bridge.validate(plan)

    def test_successful_preflight_uses_literal_paths_with_spaces_and_checks_executables(self):
        proxy = self.base / 'proxy binary'
        runtime = self.base / 'runtime binary'
        wrapper = self.base / 'launch-noita.sh'
        game = self.base / 'noita.exe'
        for file in (proxy, runtime, wrapper, game):
            file.touch()
            file.chmod(0o700)
        plan = dict(root=str(self.base),proxy=str(proxy),game_dir=str(self.base),
                    game_command=[str(runtime)],tools=[],executables=[str(runtime)],
                    runtime=str(self.base),compat_data=str(self.base))
        with patch.object(bridge,'steam_process',return_value=self.base), \
             patch.object(bridge.Path,'readlink',return_value=Path('same-pid-namespace')):
            self.assertEqual(bridge.validate(plan),self.base)
            runtime.chmod(0o600)
            with self.assertRaisesRegex(RuntimeError,'not executable'):
                bridge.validate(plan)

    def test_proxy_joins_session_environment_and_passes_an_explicit_game_handoff(self):
        plan = dict(proxy='/profile with spaces/proxy',proxy_args=['--settings-path','/profile settings'],runtime='/profile runtime')
        file = self.base / 'plan.json'
        file.write_text(json.dumps(plan))
        with patch.object(bridge,'validate',return_value=self.base), \
             patch.object(bridge,'steam_environment',return_value={'SteamAppId':'881100'}), \
             patch.object(bridge.os,'chdir'), patch.object(bridge.os,'execve') as execute, \
             patch.object(bridge.sys,'argv',['helper','proxy',str(file)]):
            bridge.main()
        program,args,env = execute.call_args.args
        self.assertEqual(program,plan['proxy'])
        self.assertEqual(args[-2:],['--launch-cmd',"'/profile runtime/launch-noita.sh'"])
        self.assertEqual(env,{'SteamAppId':'881100'})


if __name__ == '__main__':
    unittest.main()
