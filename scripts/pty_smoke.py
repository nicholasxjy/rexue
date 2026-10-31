#!/usr/bin/env python3
"""Real system-fzf acceptance in a controlling PTY; isolated Rex/GitHub fixtures."""
import argparse
import errno
import fcntl
import json
import os
import pathlib
import pty
import re
import select
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

MARKER = b'REXUE_TTY_REPORT='


def child():
    initial = termios.tcgetattr(0)
    proc = subprocess.Popen(sys.argv[2:])
    signal.signal(signal.SIGTERM, lambda *_: proc.send_signal(signal.SIGTERM))
    code = proc.wait()
    current = termios.tcgetattr(0)
    # PENDIN is kernel state (retype pending input), not terminal configuration.
    # macOS can set it on canonical-mode restoration; the next read clears it.
    # Compare every other bit, speed and control character exactly.
    pending = getattr(termios, 'PENDIN', 0)
    initial[3] &= ~pending
    current[3] &= ~pending
    os.write(1, MARKER + json.dumps({'restored': current == initial, 'exit': code}).encode() + b'\n')
    return code


class Session:
    def __init__(self, binary, cfg, root, arguments=None, env_extra=None, **_):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
        env = dict(os.environ, TERM='xterm-256color', REXUE_STATE_DIR=str(root / 'state'), XDG_CACHE_HOME=str(root / 'cache'))
        env.update(env_extra or {})
        # These must be ignored by rexue; otherwise they corrupt the row protocol.
        env.update(FZF_DEFAULT_OPTS='--multi --print-query --bind enter:abort', FZF_DEFAULT_COMMAND='exit 93')
        args = arguments if arguments is not None else ['--detach', '-C', str(cfg), 'picker', '--config']
        def setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        self.proc = subprocess.Popen([sys.executable, __file__, '--child', binary, *args], cwd=root, env=env,
                                     stdin=self.slave, stdout=self.slave, stderr=self.slave, preexec_fn=setup)
        self.capture = bytearray()

    def drain(self, timeout=0.05):
        ready, _, _ = select.select([self.master], [], [], timeout)
        if ready:
            try:
                self.capture.extend(os.read(self.master, 65536))
            except OSError as error:
                if error.errno != errno.EIO:
                    raise

    def until(self, predicate, message, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain()
            if predicate():
                return
        raise AssertionError(message + '\n' + self.text()[-2500:])

    def text(self):
        # fzf uses cursor movements, so this is an output transcript, not a grid.
        # Assertions about filtering use preview/selection effects, never absence
        # of a name in this transcript (which contains earlier frames).
        return re.sub(r'\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))', '', self.capture.decode('utf-8', 'replace'))

    def expect(self, text, timeout=8):
        self.until(lambda: text in self.text(), 'missing fzf output: ' + text, timeout)

    def send(self, data):
        os.write(self.master, data)

    def resize(self, width, height):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
        os.killpg(self.proc.pid, signal.SIGWINCH)

    def finish(self, expected=0):
        self.until(lambda: MARKER in self.capture, 'terminal restoration report missing')
        self.proc.wait(timeout=2)
        report = json.loads(re.search(MARKER + rb'([^\r\n]+)', self.capture)[1])
        assert report == {'restored': True, 'exit': expected}, (report, self.text()[-2500:])
        assert self.proc.returncode == expected
        return report

    def close(self):
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL)
        os.close(self.master)
        os.close(self.slave)
        self.proc.wait(timeout=2)


def executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/rexue')
    parser.add_argument('--output', type=pathlib.Path)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    checks = []
    with tempfile.TemporaryDirectory(prefix="rexue fzf ' ") as directory:
        root = pathlib.Path(directory)
        cfg = root / 'config.toml'
        bin_dir = root / 'bin'
        bin_dir.mkdir()
        rex = bin_dir / 'rex-fixture'
        name = "项目 same '; $(touch INJECTED) {q}"
        (root / 'fixture.json').write_text(json.dumps({'name': name, 'path': str(root)}))
        executable(rex, '#!' + sys.executable + '\n' + '''import json,pathlib,sys
root=pathlib.Path(__file__).parent.parent
fixture=json.loads((root/'fixture.json').read_text())
labels=json.loads((root/'labels.json').read_text()) if (root/'labels.json').exists() else {}
a=sys.argv[4:]
if a[:1]==['-S']:a=a[2:]
with (root/'rex-calls.jsonl').open('a') as f:f.write(json.dumps(a)+'\\n')
if a[:2]==['whoami','--json']:print('{}')
elif a[:2]==['ls','--json']:
    killed=(root/'killed').read_text().splitlines() if (root/'killed').exists() else []
    print(json.dumps({'sessions':[{'session_id':'session:'+i,'label':labels.get('session:'+i,fixture['name'])} for i in ['first','second'] if 'session:'+i not in killed]}))
elif a[:2]==['session','inspect']:
    print(json.dumps({'session_id':a[2],'label':labels.get(a[2],fixture['name']),'active_window_id':'window:fixture','windows':[{'window_id':'window:fixture','label':'Terminal','focused_block_id':'block:fixture'}]}))
elif a[:2]==['session','rename']:
    if (root/'rename-fail').exists():print('rename fixture failure',file=sys.stderr);sys.exit(7)
    labels[a[2]]=a[3]
    (root/'labels.json').write_text(json.dumps(labels))
    print('{}')
elif a[:2]==['block','call']:print(json.dumps({'foreground':{'cwd':fixture['path']}}))
elif a[0]=='capture':print('PREVIEW_'+a[2])
elif a[0]=='new':print(json.dumps({'session_id':'session:new','initial_windows':[{'block_ids':['block:fixture']}]}))
elif a[0]=='kill':
    with (root/'killed').open('a') as f:f.write(a[1]+'\\n')
    print('{}')
elif a[:2]==['client','ls']:print('{"clients":[]}')
else:
    print('unexpected fixture command: '+repr(a),file=sys.stderr);sys.exit(91)
''')
        config = 'strict_mode=true\n[frecency]\nlist_command="true"\nadd_command="true"\nquery_command="false"\n[tui]\npreview=true\npreview_min_width=50\n[[session]]\nname=' + json.dumps(name, ensure_ascii=False) + '\npath=' + json.dumps(str(root)) + '\nalias="Ä"\nalias_auto_connect=true\n'
        cfg.write_text(config)
        base = ['--rex-bin', str(rex), '--detach', '-C', str(cfg)]

        def run_case(label, arguments, interact, expected=0):
            print("PTY: " + label, flush=True)
            s = Session(binary, cfg, root, arguments=[*base, *arguments])
            try:
                interact(s)
                report = s.finish(expected)
                checks.append({'case': label, **report})
                return s.text()
            finally:
                s.close()

        def cancel(s):
            s.expect(name)
            s.expect('PREVIEW_session:first')
            s.resize(42, 12)
            s.send(b'zzzero-no-matches')
            s.expect('zzzero-no-matches')
            s.expect('0/0')
            s.send(b'\x1b')
        run_case('session-cancel-filter-preview-resize', ['picker', '--rex'], cancel)
        run_case('session-sigterm-restores-terminal', ['picker', '--rex'],
                 lambda s: (s.expect(name), s.proc.send_signal(signal.SIGTERM)))
        run_case('preview-cli-hidden-then-toggle', ['picker', '--rex', '--no-preview'],
                 lambda s: (s.expect(name), s.send(b'\x0f'), s.expect('PREVIEW_session:first'), s.send(b'\x1b')))
        assert not (root / 'killed').exists()
        for spelling in [[], ['pick'], ['pk']]:
            text = run_case('entry-' + ('default' if not spelling else spelling[0]), spelling,
                            lambda s: (s.expect(name), s.send(b'\x1b')))

        text = run_case('duplicate-name-select-second-id', ['picker', '--rex'],
                        lambda s: (s.expect(name), s.send(b'\x1b[B'), s.expect('PREVIEW_session:second'), s.send(b'\r')))
        assert 'session:second' in text
        text = run_case('unicode-alias-auto-connect', ['picker', '--rex'],
                        lambda s: (s.expect(name), s.send('/ä'.encode())))
        assert 'session:first' in text
        text = run_case('prefilled-alias-does-not-auto-connect', ['picker', '--rex', '--query=/ä'],
                        lambda s: (s.expect(name), s.expect('PREVIEW_session:first'), s.send(b'\x1b')))
        assert not text.rstrip().endswith('session:first')
        text = run_case('disable-alias-auto-and-select', ['picker', '--rex', '--no-alias-auto'],
                        lambda s: (s.expect(name), s.send('/ä'.encode()), s.expect('/ä'), s.send(b'\r')))
        assert 'session:first' in text
        text = run_case('index-shortcut-second-id', ['picker', '--rex'],
                        lambda s: (s.expect(name), s.send(b'#'), s.expect('#2'), s.send(b'2')))
        assert 'session:second' in text

        def rename_calls():
            return [call for call in (json.loads(line) for line in (root/'rex-calls.jsonl').read_text().splitlines()) if call[:2] == ['session', 'rename']]

        def resume_after(s, data):
            start = len(s.capture)
            s.send(data)
            s.until(lambda: b'Enter connect' in s.capture[start:], 'picker did not resume after rename')

        def open_rename(s):
            s.expect(name)
            s.send(b'\x12')
            s.expect('Rename Rex session')
            s.expect('New name > ')

        def cancel_rename(s):
            open_rename(s)
            s.send(b'\x15cancelled rename')
            resume_after(s, b'\x1b')
            s.send(b'\x1b')
        run_case('rename-cancel-preserves-name', ['picker', '--rex'], cancel_rename)
        run_case('rename-unchanged-is-noop', ['picker', '--rex'],
                 lambda s: (open_rename(s), resume_after(s, b'\r'), s.send(b'\x1b')))
        assert not rename_calls()

        def empty_rename(s):
            open_rename(s)
            s.send(b'\x15   \r')
            s.expect('session name cannot be empty')
            s.send(b'\x1b')
        run_case('rename-empty-name-rejected', ['picker', '--rex'], empty_rename)
        run_case('rename-config-row-rejected', ['picker', '--config'],
                 lambda s: (s.expect(name), s.send(b'\x12'), s.expect('Only live Rex sessions can be renamed'), s.send(b'\x1b')))
        assert not rename_calls()

        (root/'rename-fail').touch()
        def failed_rename(s):
            # The fixture executable has a long path; show its complete error.
            s.resize(240, 30)
            open_rename(s)
            s.send(b'\x15failed name\r')
            s.expect('Rename failed:')
            s.expect('rename fixture failure')
            s.send(b'\x1b')
        run_case('rename-backend-error-retains-list', ['picker', '--rex', '--no-preview'], failed_rename)
        (root/'rename-fail').unlink()
        assert not (root/'labels.json').exists()

        renamed = "项目 renamed '; $(touch RENAME_INJECTED) {q}"
        before_calls = len((root/'rex-calls.jsonl').read_text().splitlines())
        def rename_second(s):
            s.resize(240, 30)
            s.expect(name)
            s.send(b'\x1b[B')
            s.expect('PREVIEW_session:second')
            s.send(b'\x12')
            s.expect('Rename Rex session')
            s.send(b'\x15' + renamed.encode())
            resume_after(s, b'\r')
            s.expect('1/1')  # The original 'same' query now matches only the first row.
            start = len(s.capture)
            s.send(b'\x7f' * len('same'))
            s.until(lambda: renamed in s.capture[start:].decode('utf-8', 'replace'), 'renamed row missing after clearing query')
            s.send(b'\x1b')
        text = run_case('rename-second-duplicate-refreshes-and-keeps-query', ['picker', '--rex', '--query=same'], rename_second)
        assert json.loads((root/'labels.json').read_text()) == {'session:second': renamed}
        calls = [json.loads(line) for line in (root/'rex-calls.jsonl').read_text().splitlines()[before_calls:]]
        assert [call for call in calls if call[:2] == ['session', 'rename']] == [['session', 'rename', 'session:second', renamed]]
        assert not any(call[0] in ['new', 'focus', 'kill'] or call[:2] == ['client', 'ls'] for call in calls)
        assert not (root/'RENAME_INJECTED').exists()
        (root/'labels.json').unlink()

        def decline(s):
            s.expect(name)
            s.send(b'\x18')
            s.expect('Kill this Rex session?')
            s.send(b'n')
            # Wait for a second finder, instead of racing an Esc against teardown.
            start = len(s.capture)
            s.until(lambda: 'Enter connect' in s.capture[start:].decode('utf-8', 'replace'), 'picker did not resume')
            s.send(b'\x1b')
        run_case('remove-decline', ['picker', '--rex'], decline)
        assert not (root / 'killed').exists()
        def delete(s):
            s.expect(name)
            s.send(b'\x18')
            s.expect('Kill this Rex session?')
            s.send(b'y')
            s.until(lambda: (root/'killed').exists(), 'remove callback was not called')
            s.expect('PREVIEW_session:second')
            s.send(b'\x1b')
        run_case('remove-confirm-and-refresh', ['picker', '--rex'], delete)
        assert (root / 'killed').read_text() == 'session:first\n'
        assert not (root / 'INJECTED').exists()
        def delete_last_and_query_alias(s):
            s.expect(name)
            s.send(b'\x18')
            s.expect('Kill this Rex session?')
            s.send(b'y')
            s.until(lambda: (root/'killed').read_text().splitlines() == ['session:first', 'session:second'], 'last duplicate was not removed')
            s.expect('0/0')
            s.send('/ä'.encode())
            s.expect('/ä')
            deadline = time.monotonic() + 0.5
            s.until(lambda: time.monotonic() >= deadline or s.proc.poll() is not None, 'alias cancellation check timed out', timeout=2)
            assert s.proc.poll() is None, 'deleted alias auto-connected to a removed session'
            s.send(b'\x1b')
        run_case('last-duplicate-removal-retires-alias', ['picker', '--rex'], delete_last_and_query_alias)
        # Restore only the private fake session for the following worktree case.
        (root/'killed').write_text('session:first\n')

        # Private local repository directories + fake gh; no network or real Rex.
        repo = root / 'repo'
        repo.mkdir()
        def git(*args):
            subprocess.run(['git', '-C', str(repo), *args], capture_output=True, check=True, timeout=10)
        git('init', '-b', 'main')
        (repo/'seed').write_text('fixture\n')
        git('add', 'seed')
        git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-m', 'fixture')
        git('worktree', 'add', '-b', 'fixture-42', str(repo/'.wk'/'42'))
        executable(bin_dir / 'gh', '#!' + sys.executable + '\n' + '''import json,pathlib,sys
root=pathlib.Path(__file__).parent.parent
with (root/'gh-calls').open('a') as f:f.write('call\\n')
if (root/'gh-fail').exists():print('offline fixture',file=sys.stderr);sys.exit(7)
print(json.dumps({'data':{'repository':{'n42':{'title':'Scoped worktree title','state':'OPEN'},'issueOrPullRequest':{'__typename':'Issue','number':42,'title':'Scoped worktree title','body':''}}}}))
''')
        cfg.write_text(config + '\n[[worktree]]\nrepo="owner/repo"\npath=' + json.dumps(str(repo)) + '\nfetch=false\n')
        env = {'PATH': str(bin_dir) + os.pathsep + os.environ['PATH']}
        s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo)], env_extra=env)
        try:
            s.expect('#42')
            s.expect('Scoped worktree title')
            s.until(lambda: (root/'gh-calls').exists(), 'automatic metadata refresh did not run')
            initial_calls = len((root / 'gh-calls').read_text().splitlines())
            (root / 'gh-fail').touch()
            s.send(b'\x12')
            s.expect('Refresh failed:')
            assert len((root / 'gh-calls').read_text().splitlines()) > initial_calls
            s.send(b'zzzero-worktree')
            s.expect('zzzero-worktree')
            s.send(b'\x1b')
            checks.append({'case': 'worktree-alias-path-auto-refresh-manual-error-filter-cancel', **s.finish()})
        finally:
            s.close()

        (root/'gh-fail').unlink()
        trap = root/'alias-trap'; trap.mkdir()
        cfg.write_text(cfg.read_text()+'\n[[session]]\nname="alias-trap"\npath='+json.dumps(str(trap))+'\nalias='+json.dumps(str(repo/'.wk'/'42'))+'\n')
        (root/'fixture.json').write_text(json.dumps({'name': name, 'path': str(repo/'.wk'/'42')}))
        s = Session(binary, cfg, root, arguments=[*base, 'worktree', 'picker', '--repo', 'owner/repo', '--refresh'], env_extra=env)
        try:
            s.expect('Scoped worktree title')
            s.send(b'\r')
            report = s.finish()
            assert 'session:second' in s.text(), s.text()[-2000:]
            checks.append({'case': 'worktree-repo-selected-prepared-and-connected', **report})
        finally:
            s.close()

        saved_configuration = cfg.read_text()
        wanted, wrong = root/'wanted', root/'wrong'
        wanted.mkdir(); wrong.mkdir()
        for literal_name in ['📁 chosen', ' chosen']:
            cfg.write_text('strict_mode=true\n[frecency]\nlist_command="true"\nadd_command="true"\n[tui]\npreview=false\n' +
                           '\n'.join('[[session]]\nname='+json.dumps(n, ensure_ascii=False)+'\npath='+json.dumps(str(p)) for n,p in [(literal_name,wanted),('chosen',wrong)]))
            before_calls = len((root/'rex-calls.jsonl').read_text().splitlines())
            text = run_case('literal-config-name-'+literal_name[0], ['picker', '--config', '--query='+literal_name[0]],
                            lambda s: (s.expect(literal_name), s.send(b'\r')))
            assert 'session:new' in text
            new_calls = [json.loads(line) for line in (root/'rex-calls.jsonl').read_text().splitlines()[before_calls:]]
            creations = [call for call in new_calls if call[0]=='new']
            assert len(creations)==1 and creations[0][1]==literal_name, creations
            cwd = creations[0].index('-c')
            assert pathlib.Path(creations[0][cwd+1]).resolve()==wanted.resolve(), creations
            if literal_name[0]=='📁':
                key = 'name='+json.dumps(literal_name, ensure_ascii=False)+'\n'
                cfg.write_text(cfg.read_text().replace(key, key+'alias="hidden"\n', 1))
                before_calls = len((root/'rex-calls.jsonl').read_text().splitlines())
                text = run_case('hidden-config-alias-keeps-literal-name-and-path', ['picker', '--rex'],
                                lambda s: (s.expect(name), s.send(b'/hidden')))
                assert 'session:new' in text
                creations = [call for call in (json.loads(line) for line in (root/'rex-calls.jsonl').read_text().splitlines()[before_calls:]) if call[0]=='new']
                assert len(creations)==1 and creations[0][1]==literal_name, creations
                assert pathlib.Path(creations[0][creations[0].index('-c')+1]).resolve()==wanted.resolve(), creations

        cfg.write_text(saved_configuration)

        # Missing fzf and failed/old fzf must fail before contacting Rex.
        isolated = root / 'isolated'
        isolated.mkdir()
        before = (root/'rex-calls.jsonl').read_bytes()
        for label, script, expected in [
            ('missing-fzf', None, 'system fzf is required'),
            ('old-fzf', '#!/bin/sh\nprintf "0.60.0\\n"\n', 'fzf 0.63 or newer'),
            ('fzf-error', '#!/bin/sh\nif [ "$1" = --version ]; then printf "0.74.5\\n"; else exit 2; fi\n', 'fzf exited unexpectedly'),
        ]:
            if script is not None:
                executable(isolated/'fzf', script)
            result = subprocess.run([*([binary]+base), 'picker', '--config'], env=dict(os.environ, PATH=str(isolated)), capture_output=True, text=True, timeout=8)
            assert result.returncode == 1 and expected in result.stderr, (label, result)
            checks.append({'case': label, 'exit': result.returncode})
        # Missing/old fzf preflight made no Rex calls; failed interactive fzf only
        # fetched config rows and performed no connection or removal.
        assert (root/'rex-calls.jsonl').read_bytes() == before
    receipt = json.dumps({'system_fzf': subprocess.check_output(['fzf','--version'], text=True).strip(), 'pty': checks}, ensure_ascii=False, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(receipt + '\n')
    print(receipt)


if __name__ == '__main__':
    sys.exit(child() if len(sys.argv) > 1 and sys.argv[1] == '--child' else main())
