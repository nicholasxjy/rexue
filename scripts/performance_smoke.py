#!/usr/bin/env python3
"""Measure release picker startup and idle resources with isolated Rex fixtures."""
import argparse
import json
import pathlib
import shlex
import statistics
import subprocess
import tempfile
import time

from pty_smoke import Session, executable


def process_resources(parent):
    """Sum RSS and CPU time for the picker and its descendants, excluding our wrapper."""
    output = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss=,time='], text=True)
    rows = [line.split() for line in output.splitlines()]
    descendants = {parent}
    while True:
        children = {int(pid) for pid, ppid, *_ in rows if int(ppid) in descendants}
        if children <= descendants:
            break
        descendants.update(children)
    rss_kib = 0
    cpu_seconds = 0
    for pid, _, rss, cpu in rows:
        if int(pid) not in descendants or int(pid) == parent:
            continue
        rss_kib += int(rss)
        seconds = 0
        for part in cpu.split(':'):
            seconds = seconds * 60 + float(part)
        cpu_seconds += seconds
    return {'rss_kib': rss_kib, 'cpu_seconds': cpu_seconds}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/release/rexue')
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--idle-seconds', type=float, default=1)
    parser.add_argument('--max-rex-calls', type=int)
    parser.add_argument('--legacy-rex', action='store_true', help='exercise a Rex CLI without Lua scripting')
    parser.add_argument('--output', type=pathlib.Path)
    args = parser.parse_args()
    if args.runs < 1 or args.idle_seconds <= 0:
        parser.error('--runs and --idle-seconds must be positive')
    binary = str(pathlib.Path(args.binary).resolve())
    samples = []
    with tempfile.TemporaryDirectory(prefix='rexue-performance-') as directory:
        root = pathlib.Path(directory)
        (root / 'calls').mkdir()
        cfg = root / 'config.toml'
        cfg.write_text('[frecency]\nlist_command="true"\n[tui]\npreview=false\n')
        if args.legacy_rex:
            (root / "legacy").touch()
        sessions = []
        snapshots = {}
        for index in range(4):
            session_id = f'session:fixture-{index}'
            sessions.append({'session_id': session_id, 'label': f'project-{index}'})
            blocks = []
            metadata = {}
            for pane in range(4):
                block_id = f'block:fixture-{index}-{pane}'
                agent = pane < 3
                blocks.append({'block_id': block_id, 'creator_name': 'com.superlogical.terminal'})
                (root / f'process-{block_id}.json').write_text(json.dumps({
                    'foreground': {'pid': pane + 1, 'name': 'node' if agent else 'zsh',
                                   'argv0': 'pi' if agent else 'zsh', 'cwd': str(root)}}))
                (root / f'program_status-{block_id}.json').write_text(json.dumps({
                    'records': [{'app': 'pi', 'state': 'working',
                                 'owner': {'pid': pane + 1, 'name': 'node'}}] if agent else []}))
                (root / f'title-{block_id}.json').write_text(json.dumps({'title': f'Task {index}-{pane}'}))
                metadata[block_id] = {method: json.loads((root / f'{method}-{block_id}.json').read_text())
                                      for method in ('process', 'program_status', 'title')}
            view = {'active_window_id': f'window:{index}-00000000',
                    'windows': [{'window_id': f'window:{index}-00000000', 'label': 'Window 00000000',
                                 'focused_block_id': blocks[0]['block_id'], 'layers': [{'blocks': blocks}]}]}
            (root / f'inspect-{session_id}.json').write_text(json.dumps(view))
            snapshots[session_id] = {'view': view, 'blocks': metadata}
        (root / 'sessions.json').write_text(json.dumps({'sessions': sessions}))
        (root / 'snapshot.json').write_text(json.dumps({'sessions': sessions, 'snapshots': snapshots}))
        fixture = root / 'rex-fixture'
        executable(fixture, '#!/bin/sh\nroot=' + shlex.quote(str(root)) + '''
shift 3
if [ "$1" = '-S' ]; then shift 2; fi
printf '%s\\n' "$*" > "$root/calls/$$"
/bin/sleep 0.015
case "$1 $2" in
    'do --eval')
        if [ -f "$root/legacy" ]; then printf 'Error: unknown flag: --eval' >&2; exit 2; fi
        /bin/cat "$root/snapshot.json" ;;
    'ls --json') /bin/cat "$root/sessions.json" ;;
    'session inspect') /bin/cat "$root/inspect-$3.json" ;;
    'block call') /bin/cat "$root/$7-$6.json" ;;
    'whoami --json') printf '{}' ;;
    *) printf 'unexpected fixture request: %s\\n' "$*" >&2; exit 91 ;;
esac
''')
        for _ in range(args.runs):
            for call in (root / 'calls').iterdir():
                call.unlink()
            started = time.perf_counter()
            session = Session(binary, cfg, root, arguments=[
                '--rex-bin', str(fixture), '--detach', '-C', str(cfg), 'picker', '--rex'])
            try:
                session.expect('project-0')
                open_ms = (time.perf_counter() - started) * 1000
                # Drain the initial fzf callbacks before observing idle work.
                until = time.monotonic() + 0.15
                while time.monotonic() < until:
                    session.drain(0.01)
                calls = len(list((root / 'calls').iterdir()))
                before = process_resources(session.proc.pid)
                idle_started = time.monotonic()
                while time.monotonic() - idle_started < args.idle_seconds:
                    session.drain(0.02)
                idle_elapsed = time.monotonic() - idle_started
                after = process_resources(session.proc.pid)
                idle_calls = len(list((root / 'calls').iterdir())) - calls
                session.send(b'\x1b')
                session.finish(expected=0)
                sample = {'open_ms': round(open_ms, 2), 'open_rex_calls': calls,
                          'idle_rss_kib': after['rss_kib'],
                          'idle_cpu_percent': round(max(0, after['cpu_seconds'] - before['cpu_seconds']) / idle_elapsed * 100, 2),
                          'idle_rex_calls': idle_calls, 'terminal_restored': True}
                samples.append(sample)
                print(json.dumps(sample), flush=True)
                assert idle_calls == 0, 'idle picker made unexpected Rex requests'
                if args.max_rex_calls is not None:
                    assert calls <= args.max_rex_calls, f'{calls} Rex calls exceed {args.max_rex_calls}'
            finally:
                session.close()
    report = {
        'method': 'Four sessions, one unnamed tab and four panes per session (three agents and one shell); each Rex CLI process sleeps 15ms to model process/connection setup (individual RPC latency within Lua is not simulated). Real fzf in a PTY; preview disabled. RSS sums the picker and its descendants; CPU is the idle CPU-time delta, excluding the Python wrapper. This does not measure live app rendering or peak startup memory.',
        'runs': args.runs,
        'legacy_rex': args.legacy_rex,
        'median': {key: statistics.median(sample[key] for sample in samples)
                   for key in ('open_ms', 'open_rex_calls', 'idle_rss_kib', 'idle_cpu_percent')},
        'samples': samples,
    }
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'median': report['median']}), flush=True)


if __name__ == '__main__':
    main()
