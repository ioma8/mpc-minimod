# /// script
# requires-python = ">=3.10"
# ///
"""Prepare isolated emulator projects, run every acceptance scenario, restore state.

Run from the repository root with uv. The suite snapshots the active Temp-Project,
uses converter/project1 as its populated fixture and derives an empty fixture, then
restores the original project and preload configuration even when a case fails.
"""
import argparse
import ast
import json
import os
from pathlib import Path
import shlex
import socket
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
import urllib.request

from make_empty_project import make_empty_project

ROOT = Path(__file__).resolve().parents[2]
MOD = ROOT / 'mod'
LAB_DEFAULT = ROOT.parent / 'virtual-mpc-sample'
PROJECT = '/data/TempProjects/Temp-Project.xpj'
PROJECT_DATA = '/data/TempProjects/Temp-Project_[ProjectData]'
DROPIN = '/run/systemd/system/ac50.service.d/mpclab-preload.conf'
PRELOAD_BINARY = '/run/mpclab-preload/minimod.so'
SCREEN_OFFSET = 0x6e212a0


def symbols(path):
    found = {}
    for line in path.read_text().splitlines():
        fields = line.split()
        if fields and fields[-1] == 'g_screen':
            found['g_screen'] = int(fields[0], 16)
    if 'g_screen' not in found:
        raise RuntimeError('g_screen is missing from the linker map')
    return found


def case_settings(path):
    """Read scenario fixture declarations without importing test code."""
    settings = {}
    tree = ast.parse(path.read_text(), filename=str(path))
    for node in tree.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            key = node.targets[0].id
            if key in ('PROJECT', 'MIDI_INPUT'):
                settings[key] = ast.literal_eval(node.value)
    return settings


def cases_for(lab_path, panel):
    cases = [
        ('menu', 'populated', 'emulator_menu.py', ['--panel', panel]),
        ('autotrim', 'populated', 'emulator_autotrim.py', ['--lab', str(lab_path), '--pad', '1']),
        ('spectral', 'populated', 'emulator_spectral.py', ['--lab', str(lab_path), '--ready']),
        ('hpss', 'populated', 'emulator_hpss.py', ['--lab', str(lab_path), '--panel',
                                     panel]),
        ('reverb', 'populated', 'emulator_reverb.py', ['--lab', str(lab_path), '--panel',
                                         panel,
                                         '--effect', 'reverb']),
        ('vinyl', 'populated', 'emulator_reverb.py', ['--lab', str(lab_path), '--panel',
                                        panel,
                                        '--effect', 'vinyl']),
        ('lofi', 'populated', 'emulator_reverb.py', ['--lab', str(lab_path), '--panel',
                                       panel,
                                       '--effect', 'lofi']),
        ('bass_sample', 'populated', 'emulator_bass_sample.py', ['--lab', str(lab_path), '--panel',
                                                   panel]),
        ('midi_chromatic', 'populated', 'emulator_midi_chromatic.py', ['--lab', str(lab_path), '--panel',
                                                        panel]),
        ('pad_octave', 'populated', 'emulator_pad_octave.py', ['--lab', str(lab_path)]),
        ('microtonal', 'populated', 'emulator_microtonal.py', ['--lab', str(lab_path)]),
        ('microtonal_pitch', 'populated', 'emulator_microtonal_pitch.py', ['--lab', str(lab_path)]),
        ('looper', 'populated', 'emulator_looper.py', ['--lab', str(lab_path), '--panel',
                                         panel]),
        ('looper_lights', 'empty', 'emulator_looper_lights.py', ['--panel',
                                                       panel]),
        ('synth_lights', 'populated', 'emulator_synth_lights.py', ['--panel',
                                                     panel]),
        ('synth_shift', 'populated', 'emulator_synth_shift.py', ['--lab', str(lab_path), '--panel',
                                                   panel]),
    ]
    cases.append(('synth_empty', 'empty', 'emulator_synth_empty.py',
                  ['--lab', str(lab_path), '--panel', panel, '--pad', '1']))
    for path in sorted((MOD / 'tests' / 'cases').glob('test_*.py')):
        settings = case_settings(path)
        project = settings.get('PROJECT', 'populated')
        if not isinstance(settings.get('MIDI_INPUT', False), bool):
            raise ValueError(f'{path}: MIDI_INPUT must be True or False')
        if project not in ('populated', 'empty'):
            raise ValueError(f'{path}: PROJECT must be populated or empty')
        cases.append((path.stem.removeprefix('test_'), project, str(path),
                      ['--lab', str(lab_path), '--panel', panel]))
    names = [case[0] for case in cases]
    if len(names) != len(set(names)):
        raise ValueError('Duplicate emulator case name')
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, default=LAB_DEFAULT)
    parser.add_argument('--python', default='3.14', help='uv-managed Python version')
    parser.add_argument('cases', nargs='*', help='case names to run (default: all)')
    parser.add_argument('--only', action='append', help='run only this named case (repeatable)')
    parser.add_argument('--list', action='store_true', help='list available cases and exit')
    parser.add_argument('--no-build', action='store_true', help='reuse the existing build')
    args = parser.parse_args()
    args.lab = args.lab.resolve()
    library = MOD / 'build/minimod.so'
    mapfile = MOD / 'build/minimod.map'
    available = cases_for(args.lab, '')
    if args.list:
        for name, project, script, _ in available:
            print(f'{name:18} {project:10} {Path(script).name}')
        return 0
    requested = set(args.cases + (args.only or []))
    unknown = requested - {case[0] for case in available}
    if unknown:
        parser.error('unknown case(s): ' + ', '.join(sorted(unknown)) + '; use --list')
    if not (args.lab / 'lab.py').is_file():
        parser.error(f'lab not found at {args.lab}; clone virtual-mpc-sample next to this repository or pass --lab')
    sys.path.insert(0, str(args.lab))
    import lab
    panel = f'http://{lab.HOST}:{lab.PORTS["panel"]}'
    test_cases = cases_for(args.lab, panel)
    if requested:
        test_cases = [case for case in test_cases if case[0] in requested]
    for service, port in [('SSH', lab.PORTS['ssh']), ('panel', lab.PORTS['panel'])]:
        try:
            with socket.create_connection((lab.HOST, port), timeout=1):
                pass
        except OSError:
            parser.error(f'{service} is not reachable at {lab.HOST}:{port}; start the lab first (see mod/tests/README.md)')
    if not args.no_build:
        print('===== build minimod =====', flush=True)
        # The SDK build script invokes python3. Supply the interpreter uv chose
        # for this script, rather than relying on a system Python installation.
        with tempfile.TemporaryDirectory(prefix='minimod-python-') as bindir:
            shim = Path(bindir) / 'python3'
            shim.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' "$@"\n')
            shim.chmod(0o755)
            build_env = dict(os.environ, PATH=bindir + os.pathsep + os.environ.get('PATH', ''))
            subprocess.run([str(MOD / 'build.sh')], cwd=ROOT, env=build_env, check=True)
    if not library.is_file() or not mapfile.is_file():
        parser.error('minimod build or map is missing; remove --no-build to build it')
    g_screen = symbols(mapfile)['g_screen']

    env = os.environ.copy()
    uv = shutil.which('uv')
    if not uv:
        parser.error('uv is required; install uv and rerun this suite')
    python_version = args.python

    def ssh(command, *, check=True):
        return subprocess.run(
            ['ssh', *lab.ssh_base(), 'root@' + lab.HOST, command],
            check=check, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=20).stdout

    # The fixtures and backups must be visible through the lab's 9p share.
    # Check that before stopping the app or touching its project.
    probe = Path(lab.SHARE) / f'.minimod-share-{uuid.uuid4().hex}'
    try:
        probe.write_text('minimod')
        ssh(f'test "$(cat {shlex.quote("/lab/" + probe.name)})" = minimod')
    except (OSError, subprocess.CalledProcessError) as error:
        parser.error(f'lab SSH or shared directory is unavailable: {error}; see mod/tests/README.md')
    finally:
        probe.unlink(missing_ok=True)

    def wait_process(timeout=45, require_mod=True):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                pid = int(ssh('pidof "MPC Sample"').split()[0])
                maps = ssh(f'cat /proc/{pid}/maps').decode().splitlines()
                app_map = next(line for line in maps if line.endswith('/MPC Sample'))
                mod_maps = [line for line in maps if line.endswith('/minimod.so')]
                if require_mod and not mod_maps:
                    time.sleep(.1)
                    continue
                return pid, maps
            except (subprocess.CalledProcessError, StopIteration, ValueError, IndexError):
                pass
            time.sleep(.1)
        raise TimeoutError('MPC Sample process did not become ready')

    def tap_sample():
        url = f'http://{lab.HOST}:{lab.PORTS["panel"]}/control'
        for action in ('press', 'release'):
            request = urllib.request.Request(
                url, json.dumps({'name': 'SAMPLE', 'action': action, 'value': 100}).encode(),
                {'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, timeout=5) as response:
                if not json.load(response).get('ok'):
                    raise RuntimeError('Panel rejected SAMPLE button input')
            time.sleep(.04)

    def wait_sample_screen(timeout=45):
        # On boot the app may restore a different page. The test contract is the
        # startup Temp-Project loaded by ac50 plus the Sample page active.
        deadline = time.monotonic() + timeout
        # Do not send SAMPLE as soon as the process exists: ac50 can take a few
        # seconds to construct its screen tree, and the MCU drops early input.
        while time.monotonic() < deadline:
            try:
                pid, maps = wait_process(timeout=1)
                mod_map = next(line for line in maps if line.endswith('/minimod.so'))
                mod_base = int(mod_map.split('-')[0], 16)
                screen = int.from_bytes(ssh(
                    f'dd if=/proc/{pid}/mem bs=1 skip={mod_base + g_screen} count=8 2>/dev/null'),
                    'little')
                if screen:
                    break
            except (subprocess.CalledProcessError, StopIteration, ValueError, TimeoutError):
                pass
            time.sleep(.1)
        else:
            raise TimeoutError('MPC screen tree did not initialize')
        tap_sample()
        while time.monotonic() < deadline:
            try:
                pid, maps = wait_process(timeout=1)
                mod_map = next(line for line in maps if line.endswith('/minimod.so'))
                app_map = next(line for line in maps if line.endswith('/MPC Sample'))
                mod_base = int(mod_map.split('-')[0], 16)
                app_base = int(app_map.split('-')[0], 16)
                address = mod_base + g_screen
                screen = int.from_bytes(ssh(
                    f'dd if=/proc/{pid}/mem bs=1 skip={address} count=8 2>/dev/null'), 'little')
                if screen:
                    vtable = int.from_bytes(ssh(
                        f'dd if=/proc/{pid}/mem bs=1 skip={screen} count=8 2>/dev/null'), 'little')
                    if vtable == app_base + SCREEN_OFFSET:
                        return
            except (subprocess.CalledProcessError, StopIteration, ValueError, TimeoutError):
                pass
            time.sleep(.1)
        raise TimeoutError('Sample screen did not become ready after app start')

    def start_and_wait():
        ssh('systemctl reset-failed ac50; systemctl start ac50')
        wait_sample_screen()

    def stop_app():
        ssh('systemctl stop ac50')

    def install_startup_project(kind):
        source = fixture_guest + '/' + kind
        stop_app()
        ssh(f'mkdir -p /data/TempProjects && cp -a {shlex.quote(source + "/Temp-Project.xpj")} {shlex.quote(PROJECT)} && '
            f'rm -rf {shlex.quote(PROJECT_DATA)} && '
            f'cp -a {shlex.quote(source + "/Temp-Project_[ProjectData]")} {shlex.quote(PROJECT_DATA)}')
        start_and_wait()

    def configure_midi_input():
        # MIDI CONFIG is reached on the real surface with SHIFT+PAD 8.
        control('SHIFT', 'press', 127)
        tap_sample_button('PAD 8')
        control('SHIFT', 'release')
        # The emulator starts each app with Pad MIDI In off. Move to row 4,
        # enter value editing, turn it on, and commit before the test sends MIDI.
        for _ in range(3):
            control('DATA', 'turn', 1)
        tap_sample_button('DATA')
        control('DATA', 'turn', 1)
        tap_sample_button('DATA')
        tap_sample_button('SCREEN 1')  # MIDI Config's Back soft key commits the edit.

    def control(name, action, value=0):
        request = urllib.request.Request(
            f'http://{lab.HOST}:{lab.PORTS["panel"]}/control',
            json.dumps({'name': name, 'action': action, 'value': value}).encode(),
            {'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            if not json.load(response).get('ok'):
                raise RuntimeError(f'Panel rejected {name} {action}')
        time.sleep(.08 if action == 'turn' else .1)

    def tap_sample_button(name):
        control(name, 'press', 127)
        control(name, 'release', 0)

    fixture_id = uuid.uuid4().hex[:10]
    fixture_host = Path(lab.SHARE) / f'.minimod-emulator-{fixture_id}'
    fixture_guest = '/lab/' + fixture_host.name
    fixture_host.mkdir(parents=True)
    populated_host = fixture_host / 'populated'
    empty_host = fixture_host / 'empty'
    populated_host.mkdir()
    empty_host.mkdir()

    source_project = ROOT / 'converter/project1/bapap1.xpj'
    source_data = ROOT / 'converter/project1/bapap1_[ProjectData]'
    shutil.copy2(source_project, populated_host / 'Temp-Project.xpj')
    shutil.copytree(source_data, populated_host / 'Temp-Project_[ProjectData]')
    make_empty_project(populated_host / 'Temp-Project.xpj', empty_host / 'Temp-Project.xpj')
    (empty_host / 'Temp-Project_[ProjectData]').mkdir()

    backup_ready = False
    was_running = False
    failures = []
    started = time.monotonic()

    try:
        # A fresh lab may have no startup project yet. Record presence as well
        # as contents, and restore its initial service state on every exit.
        was_running = subprocess.run(
            ['ssh', *lab.ssh_base(), 'root@' + lab.HOST,
             'systemctl is-active --quiet ac50'],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20).returncode == 0
        backup = shlex.quote(fixture_guest + '/backup')
        ssh(f'mkdir -p {backup} && systemctl stop ac50')
        try:
            ssh(f'if [ -e {shlex.quote(PROJECT)} ]; then cp -a {shlex.quote(PROJECT)} {backup}/project && touch {backup}/project.present; fi && '
                f'if [ -e {shlex.quote(PROJECT_DATA)} ]; then cp -a {shlex.quote(PROJECT_DATA)} {backup}/data && touch {backup}/data.present; fi && '
                f'if [ -f {DROPIN} ]; then cp -a {DROPIN} {backup}/preload.conf && touch {backup}/preload.present; fi && '
                f'if [ -f {PRELOAD_BINARY} ]; then cp -a {PRELOAD_BINARY} {backup}/minimod.so && touch {backup}/binary.present; fi')
        except Exception:
            if was_running:
                ssh('systemctl start ac50', check=False)
            raise
        backup_ready = True


        shutil.copy2(library, fixture_host / 'minimod.so')
        ssh('mkdir -p /run/mpclab-preload /run/systemd/system/ac50.service.d && '
            f'cp {shlex.quote(fixture_guest + "/minimod.so")} {PRELOAD_BINARY} && '
            f'printf "[Service]\\nEnvironment=LD_PRELOAD=/run/mpclab-preload/minimod.so\\n" > {DROPIN} && '
            'systemctl daemon-reload')
        for name, startup_project, script, script_args in test_cases:
            print(f'===== {name} =====', flush=True)
            case_start = time.monotonic()
            try:
                install_startup_project(startup_project)
                # The screen pointer is ready before the panel accepts input.
                time.sleep(.6)
                if name == 'midi_chromatic' or (Path(script).is_absolute() and case_settings(Path(script)).get('MIDI_INPUT', False)):
                    configure_midi_input()
                command = [uv, 'run', '--no-project', '--python', python_version,
                           '--script', str(MOD / 'tests' / script), *script_args]
                result = subprocess.run(command, cwd=ROOT, env=env)
                elapsed = time.monotonic() - case_start
                if result.returncode:
                    failures.append(name)
                    try:
                        with urllib.request.urlopen(
                                f'http://{lab.HOST}:{lab.PORTS["panel"]}/screen.png', timeout=5) as response:
                            screenshot = Path(tempfile.gettempdir()) / f'minimod-{name}-failure.png'
                            screenshot.write_bytes(response.read())
                        print(f'Failure screen: {screenshot}', flush=True)
                    except Exception:
                        pass
                    print(f'FAIL {name}: exit {result.returncode} ({elapsed:.1f}s)', flush=True)
                else:
                    print(f'PASS {name} ({elapsed:.1f}s)', flush=True)
            except Exception as error:
                failures.append(name)
                print(f'FAIL {name}: {error}', flush=True)

    finally:
        if backup_ready:
            try:
                stop_app()
                backup = shlex.quote(fixture_guest + '/backup')
                ssh(f'if [ -f {backup}/project.present ]; then cp -a {backup}/project {shlex.quote(PROJECT)}; '
                    f'else rm -f {shlex.quote(PROJECT)}; fi && '
                    f'rm -rf {shlex.quote(PROJECT_DATA)} && '
                    f'if [ -f {backup}/data.present ]; then cp -a {backup}/data {shlex.quote(PROJECT_DATA)}; fi && '
                    f'if [ -f {backup}/binary.present ]; then cp -a {backup}/minimod.so {PRELOAD_BINARY}; '
                    f'else rm -f {PRELOAD_BINARY}; fi && '
                    f'if [ -f {backup}/preload.present ]; then cp -a {backup}/preload.conf {DROPIN}; '
                    f'else rm -f {DROPIN}; fi && systemctl daemon-reload && systemctl reset-failed ac50')
                if was_running:
                    ssh('systemctl start ac50')
                    wait_process(require_mod=False)
            except Exception as error:
                print(f'RESTORE FAILED; backup preserved at {fixture_guest}/backup: {error}',
                      file=sys.stderr, flush=True)
                raise
            else:
                shutil.rmtree(fixture_host, ignore_errors=True)

    elapsed = time.monotonic() - started
    case_count = len(test_cases)
    print(f'\nEmulator suite: {case_count - len(failures)}/{case_count} passed; '
          f'{elapsed:.1f}s total (including fixtures, restarts, and restore).', flush=True)
    if failures:
        print('Failed cases: ' + ', '.join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
