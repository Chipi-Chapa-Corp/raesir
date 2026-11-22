#!/usr/bin/python3
"""Raesir's bundled Steam-session bridge. No downloaded script is evaluated."""
import json
import os
import shlex
from pathlib import Path
import sys


def steam_process(root, proc=Path('/proc')):
    root = root.resolve()
    for directory in proc.iterdir():
        if not directory.name.isdigit():
            continue
        try:
            if directory.stat().st_uid != os.getuid():
                continue
            if (directory / 'comm').read_text().strip() != 'steam':
                continue
            executable = (directory / 'exe').resolve(strict=True)
            if executable.is_relative_to(root):
                return directory
        except (OSError, RuntimeError):
            continue
    raise RuntimeError('No running Steam process owns this library. Start Steam and retry.')


def steam_environment(process):
    # flatpak enter joins namespaces but does not recreate the desktop session
    # environment. Read it here, inside Steam's PID namespace; never log it.
    environment = {}
    for entry in (process / 'environ').read_bytes().split(b'\0'):
        key, separator, value = entry.partition(b'=')
        if separator:
            environment[os.fsdecode(key)] = os.fsdecode(value)
    for key in ('LD_PRELOAD', 'LD_LIBRARY_PATH', 'LD_AUDIT', 'STEAM_RUNTIME',
                'STEAM_RUNTIME_LIBRARY_PATH'):
        environment.pop(key, None)
    # The native proxy uses this namespace's libraries, not Steam's scout overlay.
    if Path('/.flatpak-info').exists():
        environment['PATH'] = '/app/bin:/app/utils/bin:/usr/bin:/bin'
    environment['SteamAppId'] = '881100'
    environment['SteamGameId'] = '881100'
    return environment


def validate(plan):
    process = steam_process(Path(plan['root']))
    if (process / 'ns/pid').readlink() != Path('/proc/self/ns/pid').readlink():
        raise RuntimeError('Steam uses a different PID namespace. Launch the companion inside its existing Steam session.')
    required = [plan['proxy'], str(Path(plan['game_dir']) / 'noita.exe'),
                plan['game_command'][0], str(Path(plan['runtime']) / 'launch-noita.sh')]
    required.extend(str(Path(tool) / 'toolmanifest.vdf') for tool in plan['tools'])
    for path in required:
        if not Path(path).is_file() or not os.access(path, os.R_OK):
            raise RuntimeError(f'Steam cannot access a required file: {path}. Check its library/sandbox permissions.')
    for path in [plan['proxy'], *plan['executables']]:
        if not os.access(path, os.X_OK):
            raise RuntimeError(f'A required program is not executable in the Steam session: {path}')
    if not os.access(plan['runtime'], os.W_OK) or not Path(plan['compat_data']).is_dir():
        raise RuntimeError('Steam cannot access the mod profile or Noita Proton prefix.')
    return process


def game_environment(plan, environment):
    environment = dict(environment)
    environment.update({
        'SteamAppId': '881100', 'SteamGameId': '881100', 'STEAM_COMPAT_APP_ID': '881100',
        'STEAM_COMPAT_CLIENT_INSTALL_PATH': plan['root'],
        'STEAM_COMPAT_DATA_PATH': plan['compat_data'],
        'STEAM_COMPAT_INSTALL_PATH': plan['game_dir'],
        'STEAM_COMPAT_LIBRARY_PATHS': ':'.join(plan['libraries']),
        'STEAM_COMPAT_TOOL_PATHS': ':'.join(plan['tools']),
    })
    return environment


def main():
    mode, file = sys.argv[1:3]
    with open(file, encoding='utf-8') as stream:
        plan = json.load(stream)
    if mode == 'probe':
        validate(plan)
        print('steam-session-ready')
    elif mode == 'proxy':
        process = validate(plan)
        environment = steam_environment(process)
        os.chdir(Path(plan['proxy']).parent)
        command = [plan['proxy'], *plan['proxy_args'], '--launch-cmd',
                   shlex.quote(str(Path(plan['runtime']) / 'launch-noita.sh'))]
        print('Raesir: launching companion in the owning Steam session.', flush=True)
        os.execve(command[0], command, environment)
    elif mode == 'game':
        # Inherit NP_NOITA_ADDR and other networking state from the proxy. A
        # fresh Steam URL launch would lose that per-lobby environment.
        environment = game_environment(plan, os.environ)
        os.chdir(plan['game_dir'])
        command = [*plan['game_command'], *sys.argv[3:]]
        print('Raesir: launching Noita with the recorded Proton and its required Steam runtimes.', flush=True)
        os.execve(command[0], command, environment)
    else:
        raise RuntimeError('Invalid Steam companion operation')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(f'Raesir: {error}', file=sys.stderr)
        sys.exit(1)
