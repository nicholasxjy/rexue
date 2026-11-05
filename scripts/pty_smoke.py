#!/usr/bin/env python3
"""Real system-fzf acceptance in a controlling PTY; isolated Rex/Worktrunk fixtures."""
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
    def __init__(self, binary, cfg, root, arguments=None, env_extra=None, cwd=None, **_):
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
        self.proc = subprocess.Popen([sys.executable, __file__, '--child', binary, *args], cwd=cwd or root, env=env,
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
if (root/'agents-mode').exists():
    if a[:2]==['ls','--json']:
        print(json.dumps({'sessions':[{'session_id':'session:agents','label':'agent-project'}]}));sys.exit(0)
    if a[:2]==['session','inspect']:
        windows=[]
        for window,blocks in [('shell',['shell']),('agents',['first','second'])]:
            windows.append({'window_id':'window:'+window,'label':window,'focused_block_id':'block:'+blocks[0],
                            'layers':[{'blocks':[{'block_id':'block:'+b,'creator_name':'com.superlogical.terminal'} for b in blocks]}]})
        print(json.dumps({'active_window_id':'window:shell','windows':windows}));sys.exit(0)
    if a[:2]==['block','call']:
        block=a[5]
        pid=2 if block=='block:first' else 3
        if a[6]=='process':
            print(json.dumps({'last_exit':None,'foreground':{'pid':pid,'name':'node' if block!='block:shell' else 'zsh','argv0':'pi' if block!='block:shell' else 'zsh','cwd':fixture['path']}}))
        else:
            print(json.dumps({'records':[{'app':'pi','state':'working' if block=='block:first' else 'blocked','owner':{'pid':pid,'name':'node'}}] if block!='block:shell' else []}))
        sys.exit(0)
    if a[0]=='capture':print('AGENT_PREVIEW_'+a[4]);sys.exit(0)
    if a[0]=='focus':print('{}');sys.exit(0)
if a[:2]==['whoami','--json']:
    print(json.dumps({'session_id':(root/'current-session').read_text()}) if (root/'current-session').exists() else '{}')
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

        (root/'current-session').write_text('session:second')
        text = run_case('initial-focus-current-session-full-id-with-duplicate-names', ['picker', '--rex'],
                        lambda s: (s.expect('PREVIEW_session:second'), s.send(b'\r')))
        assert re.search(r'session:second\r?\nREXUE_TTY_REPORT=', text), text[-2000:]
        (root/'labels.json').write_text(json.dumps({'session:first': 'first unique', 'session:second': 'second unique'}))
        def current_session_search(s):
            s.expect('PREVIEW_session:second')
            offset = len(s.capture)
            s.send(b'first unique')
            s.expect('1/1')
            s.until(lambda: b'PREVIEW_session:first' in s.capture[offset:], 'search did not focus its matching result')
            offset = len(s.capture)
            s.send(b'\x7f' * len('first unique'))
            s.until(lambda: b'2/2' in s.capture[offset:], 'clearing search did not restore both sessions')
            s.send(b'\r')
        text = run_case('initial-focus-does-not-repeat-after-search', ['picker', '--rex'], current_session_search)
        assert re.search(r'session:first\r?\nREXUE_TTY_REPORT=', text), text[-2000:]
        (root/'labels.json').unlink()
        text = run_case('prefilled-query-keeps-first-search-result', ['picker', '--rex', '--query=same'],
                        lambda s: (s.expect('PREVIEW_session:first'), s.send(b'\r')))
        assert re.search(r'session:first\r?\nREXUE_TTY_REPORT=', text), text[-2000:]
        (root/'current-session').write_text('session:missing')
        run_case('initial-focus-missing-session-falls-back-to-first', ['picker', '--rex'],
                 lambda s: (s.expect('PREVIEW_session:first'), s.send(b'\x1b')))
        (root/'current-session').unlink()

        (root/'agents-mode').touch()
        def agent_navigation(s):
            s.expect('── Agents ──')
            s.expect('3/3')
            s.expect('pi · pane 1')
            s.expect('pi · pane 2')
            s.expect('AGENT_PREVIEW_block:shell')
            s.send(b'\x1b[B')
            s.expect('AGENT_PREVIEW_block:first')
            s.send(b'\x1b[B')
            s.expect('AGENT_PREVIEW_block:second')
            s.send(b'\r')
        run_case('agents-after-session-group-inactive-tab-duplicate-names-exact-pane', ['picker', '--agents', '--rex'], agent_navigation)
        calls=[json.loads(line) for line in (root/'rex-calls.jsonl').read_text().splitlines()]
        assert ['focus','-s','session:agents','block:second'] in calls,calls

        def agent_guards(s):
            s.expect('AGENT_PREVIEW_block:first')
            s.send(b'\x18')
            s.expect('Agent rows cannot be removed')
            s.send(b'\x12')
            s.expect('Only live Rex sessions can be renamed')
            s.send(b'\x1b')
        run_case('agents-reject-session-delete-and-rename-actions', ['picker', '--agents'], agent_guards)
        run_case('agents-filter-by-state-and-connect-pane', ['picker', '--agents'],
                 lambda s: (s.expect('── Agents ──'), s.send(b'blocked'), s.expect('1/1'), s.expect('AGENT_PREVIEW_block:second'), s.send(b'\r')))
        assert not (root/'killed').exists()
        (root/'agents-mode').unlink()

        cfg.write_text(config.replace('[tui]\n', '[tui]\ngroup_separator=true\n') + 'preview_command="printf GROUP_CONFIG_PREVIEW"\n')
        def grouped_navigation(s):
            s.expect('── rex ──')
            s.expect('── config ──')
            s.expect('3/3')
            s.expect('PREVIEW_session:first')
            s.send(b'\x1b[B')
            s.expect('PREVIEW_session:second')
            s.send(b'\x1b[B')
            s.expect('GROUP_CONFIG_PREVIEW')
            s.send(b'\x1b[A')
            s.send(b'\r')
        text = run_case('group-separator-lines-skip-headings-and-preserve-selection', ['picker', '--rex', '--config'], grouped_navigation)
        assert 'session:second' in text
        cfg.write_text(config)

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

        custom_keys = '''
[tui.keybindings]
accept = ['alt-a', 'alt-s']
cancel = ['alt-q', 'alt-z']
up = ['alt-k', 'alt-p']
down = ['alt-j', 'alt-n']
toggle_preview = ['alt-o', 'alt-v']
rename = ['alt-r', 'f2']
remove = ['alt-x', 'alt-d']
select_2 = ['alt-2', 'alt-3']
confirm_yes = ['alt-y', 'alt-v']
confirm_no = ['alt-n', 'alt-c']
'''
        cfg.write_text(config + custom_keys)

        def send_expect(s, data, expected):
            offset = len(s.capture)
            s.send(data)
            s.until(lambda: expected in s.capture[offset:].decode('utf-8', 'replace'), 'missing output after configured key: ' + expected)

        def custom_navigation(s):
            s.resize(240, 30)
            s.expect('Alt-A connect')
            s.expect('Alt-R rename')
            s.expect('PREVIEW_session:first')
            for down, up in [(b'\x1bj', b'\x1bk'), (b'\x1bn', b'\x1bp')]:
                send_expect(s, down, 'PREVIEW_session:second')
                send_expect(s, up, 'PREVIEW_session:first')
            s.send(b'\x1bz')
        run_case('configured-multiple-navigation-keys-and-header', ['picker', '--rex'], custom_navigation)
        for key in [b'\x1ba', b'\x1bs']:
            text = run_case('configured-accept-' + key.hex(), ['picker', '--rex'], lambda s, key=key: (s.expect('PREVIEW_session:first'), s.send(key)))
            assert 'session:first' in text
        for key in [b'\x1b2', b'\x1b3']:
            text = run_case('configured-index-' + key.hex(), ['picker', '--rex', '--query=#'], lambda s, key=key: (s.expect('#2'), s.send(key)))
            assert 'session:second' in text
        for key in [b'\x1bo', b'\x1bv']:
            run_case('configured-preview-' + key.hex(), ['picker', '--rex', '--no-preview'], lambda s, key=key: (s.expect('Alt-O preview'), s.send(key), s.expect('PREVIEW_session:first'), s.send(b'\x1bq')))
        for key in [b'\x1br', b'\x1bOQ']:
            def configured_rename(s, key=key):
                s.expect('PREVIEW_session:first')
                send_expect(s, key, 'New name >')
                s.expect('Alt-A save')
                send_expect(s, b'\x1ba', 'Alt-A connect')
                s.send(b'\x1bq')
            run_case('configured-rename-and-input-accept-' + key.hex(), ['picker', '--rex'], configured_rename)
        for key, no in [(b'\x1bx', b'\x1bn'), (b'\x1bd', b'\x1bc')]:
            def configured_decline(s, key=key, no=no):
                s.expect('PREVIEW_session:first')
                send_expect(s, key, 'Kill this Rex session?')
                send_expect(s, no, 'Alt-A connect')
                s.send(b'\x1bq')
            run_case('configured-remove-and-decline-' + key.hex(), ['picker', '--rex'], configured_decline)
        for yes in [b'\x1by', b'\x1bv']:
            def configured_confirm(s, yes=yes):
                s.expect('PREVIEW_session:first')
                send_expect(s, b'\x1bx', 'Kill this Rex session?')
                s.send(yes)
                s.until(lambda: (root/'killed').exists(), 'configured confirmation did not remove the session')
                s.expect('Alt-A connect')
                s.send(b'\x1bq')
            run_case('configured-confirm-yes-' + yes.hex(), ['picker', '--rex'], configured_confirm)
            assert (root/'killed').read_text() == 'session:first\n'
            (root/'killed').unlink()

        run_case('overridden-default-keys-no-longer-trigger-actions', ['picker', '--rex'],
                 lambda s: (s.expect('PREVIEW_session:first'), s.send(b'\x12\x18\r\x1b\x1bz')))
        assert not (root/'killed').exists()
        cfg.write_text(config + '\n[tui.keybindings]\nrename=[]\ntoggle_preview=[]\nselect_2=[]\n')
        def disabled_shortcuts(s):
            s.expect('PREVIEW_session:first')
            s.send(b'\x12\x0f')
            send_expect(s, b'#2', '#2')
            s.send(b'\x1b')
        run_case('empty-key-arrays-disable-actions-and-restore-digit-input', ['picker', '--rex'], disabled_shortcuts)
        cfg.write_text(config)

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

        # Real Worktrunk/Git with an isolated user config; no network or real Rex.
        import shutil
        repo = root / 'repo'
        repo.mkdir()
        def git(*args):
            subprocess.run(['git', '-C', str(repo), *args], capture_output=True, check=True, timeout=10)
        git('init', '-b', 'main')
        (repo/'seed').write_text('fixture\n')
        git('add', 'seed')
        git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-m', 'fixture')
        tree = root/'custom checkout'
        git('worktree', 'add', '-b', 'feature/scoped', str(tree))
        real_wt = shutil.which('wt')
        assert real_wt, 'Worktrunk >=0.81.0 is required'
        wt_config=root/'wt.toml'
        wt_config.write_text('worktree-path = "../picker trees/{{ branch | sanitize }}"\npre-switch = "printf switched >> {{ repo_path }}/picker-switch-hooks"\npre-start = "printf created >> {{ repo_path }}/picker-create-hooks"\n')
        executable(bin_dir/'wt-wrapper', '#!'+sys.executable+'\n'+'''import os,pathlib,sys
root=pathlib.Path(__file__).parent.parent
if (root/'wt-fail').exists() and sys.argv[1:2]==['list']:
 print('offline wt fixture',file=sys.stderr);sys.exit(7)
if (root/'wt-empty').exists() and sys.argv[1:2]==['list']:
 print((root/'wt-empty').read_text());sys.exit(0)
os.execv('''+repr(real_wt)+''',['wt','--config','''+repr(str(wt_config))+''',*sys.argv[1:]])
''')
        cfg.write_text(config + '\n[[worktree]]\npath=' + json.dumps(str(repo)) + '\n')
        env = {'PATH': str(bin_dir) + os.pathsep + os.environ['PATH'], 'REXUE_WT':str(bin_dir/'wt-wrapper')}
        worktree_config = cfg.read_text()
        cfg.write_text(worktree_config + '\n[default_session]\npreview_command="printf CHECKOUT_PREVIEW_; git -C {} branch --show-current"\n')
        nested = repo/'nested'; nested.mkdir()
        linked = root/'repo link'; linked.symlink_to(nested, target_is_directory=True)
        for label, path_args, cwd, branch in [
            ('explicit-path', ['--path', str(repo)], root, 'main'),
            ('cwd-subdirectory', [], nested, 'main'),
            ('symlink-subdirectory', ['--path', str(linked)], root, 'main'),
            ('prefilled-query', ['--path', str(repo), '--query=feature/scoped'], root, 'feature/scoped'),
        ]:
            s = Session(binary, cfg, root, arguments=[*base, 'worktree', 'picker', *path_args], env_extra=env, cwd=cwd)
            try:
                s.expect('CHECKOUT_PREVIEW_' + branch)
                s.send(b'\x1b')
                checks.append({'case': 'initial-focus-worktree-' + label, **s.finish()})
            finally:
                s.close()
        cfg.write_text(worktree_config)
        empty_hint = 'Not a Git repository or no worktrees found'
        before_empty = (root/'rex-calls.jsonl').read_bytes()
        for path_args in [[], ['--path', str(root)]]:
            s = Session(binary, cfg, root, arguments=[*base, 'worktree', 'picker', *path_args], env_extra=env)
            try:
                s.expect(empty_hint)
                s.expect('0/0')
                s.send(b'\x12')
                s.expect('Refresh failed:')
                s.send(b'\x1b')
                checks.append({'case': 'worktree-non-repository-' + ('explicit-path' if path_args else 'cwd'), **s.finish()})
            finally:
                s.close()
        for schema, empty in [('schema1', []), ('schema2-branches-only', {'schema': 2, 'items': [{'branch': 'remote-only'}]})]:
            (root/'wt-empty').write_text(json.dumps(empty))
            s = Session(binary, cfg, root, arguments=[*base, 'worktree', 'picker', '--path', str(repo)], env_extra=env)
            try:
                s.expect(empty_hint)
                s.expect('0/0')
                (root/'wt-empty').unlink()
                offset = len(s.capture)
                s.send(b'\x12')
                s.until(lambda: b'feature/scoped' in s.capture[offset:], 'worktree refresh did not recover rows')
                s.expect('2/2')
                (root/'wt-empty').write_text(json.dumps(empty))
                offset = len(s.capture)
                s.send(b'\x12')
                s.until(lambda: empty_hint.encode() in s.capture[offset:], 'empty refresh did not show repository hint')
                s.until(lambda: b'0/0' in s.capture[offset:], 'empty refresh retained selectable rows')
                s.send(b'\x1b')
                checks.append({'case': 'worktree-empty-list-refresh-recovers-' + schema, **s.finish()})
            finally:
                s.close()
                (root/'wt-empty').unlink(missing_ok=True)
        assert (root/'rex-calls.jsonl').read_bytes() == before_empty
        s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo)], env_extra=env)
        try:
            s.expect('feature/scoped')
            s.send(b'custom checkout')
            s.expect('custom checkout')
            s.expect('1/1')
            (root/'wt-fail').touch()
            s.send(b'\x12')
            s.expect('Refresh failed:')
            s.send(b'zzzero-worktree')
            s.expect('zzzero-worktree')
            s.send(b'\x1b')
            checks.append({'case': 'worktree-generic-branch-path-refresh-error-filter-cancel', **s.finish()})
        finally:
            s.close()
        (root/'wt-fail').unlink()
        trap = root/'alias-trap'; trap.mkdir()
        cfg.write_text(cfg.read_text()+'\n[[session]]\nname="alias-trap"\npath='+json.dumps(str(trap))+'\nalias='+json.dumps(str(tree))+'\n')
        (root/'fixture.json').write_text(json.dumps({'name': name, 'path': str(tree)}))
        s = Session(binary, cfg, root, arguments=[*base, 'worktree', 'picker', '--path', str(repo), '--query=feature/scoped', '--refresh'], env_extra=env)
        try:
            s.expect('feature/scoped')
            s.send(b'\r')
            report = s.finish()
            assert 'session:second' in s.text(), s.text()[-2000:]
            checks.append({'case': 'worktree-selected-path-connects-without-alias-hijack', **report})
        finally:
            s.close()
        s = Session(binary, cfg, root, arguments=[*base, 'picker', '--query=feature/scoped'], env_extra=env)
        try:
            s.expect('feature/scoped'); s.send(b'\r')
            report = s.finish()
            assert 'session:second' in s.text(), s.text()[-2000:]
            checks.append({'case':'global-worktree-source-connects-actual-checkout',**report})
        finally:
            s.close()
        saved_worktree_config = cfg.read_text()
        cfg.write_text(saved_worktree_config + '''
[tui.keybindings]
create_worktree = ['alt-w', 'alt-b']
refresh = ['alt-r', 'alt-f']
accept = ['alt-a', 'alt-s']
cancel = ['alt-q', 'alt-z']
''')
        for create, refresh_key, cancel_key in [(b'\x1bw', b'\x1br', b'\x1bq'), (b'\x1bb', b'\x1bf', b'\x1bz')]:
            s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo), '--query=zz-no-current'], env_extra=env)
            try:
                s.expect('0/0')
                send_expect(s, refresh_key, 'Alt-W create')
                send_expect(s, create, 'Branch >')
                s.expect('Alt-A create and connect')
                send_expect(s, cancel_key, 'Alt-W create')
                s.expect('zz-no-current')
                s.send(cancel_key)
                checks.append({'case': 'configured-worktree-create-refresh-empty-results-' + create.hex(), **s.finish()})
            finally:
                s.close()
        cfg.write_text(saved_worktree_config)

        def open_branch_prompt(session):
            offset = len(session.capture)
            session.send(b'\x17')
            session.until(lambda: b'Branch >' in session.capture[offset:], 'Ctrl-W branch prompt missing')
        def created_calls():
            return [json.loads(line) for line in (root/'rex-calls.jsonl').read_text().splitlines() if json.loads(line)[0]=='new']
        before_create = len(created_calls())
        s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo), '--query=zz-no-current'], env_extra=env)
        try:
            s.expect('0/0'); open_branch_prompt(s)
            s.send(b'feature/canceled')
            offset = len(s.capture); s.send(b'\x1b')
            s.until(lambda: b'Enter connect' in s.capture[offset:], 'picker did not reopen after canceling create')
            s.expect('zz-no-current'); s.send(b'\x1b')
            checks.append({'case':'worktree-create-cancel-from-empty-results',**s.finish()})
            assert len(created_calls()) == before_create
            assert subprocess.run(['git','-C',str(repo),'show-ref','--verify','refs/heads/feature/canceled'],capture_output=True).returncode != 0
        finally:
            s.close()
        s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo)], env_extra=env)
        try:
            s.expect('feature/scoped'); open_branch_prompt(s); s.send(b'\r')
            s.expect('Branch name must not be empty')
            open_branch_prompt(s); s.send(b'main\r')
            s.expect('Create failed:'); s.send(b'\x1b')
            checks.append({'case':'worktree-create-empty-and-existing-branch-errors-recover',**s.finish()})
            assert len(created_calls()) == before_create
        finally:
            s.close()
        (repo/'picker-switch-hooks').write_text('')
        (repo/'picker-create-hooks').write_text('')
        s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo), '--query=zz-no-current'], env_extra=env)
        try:
            s.expect('0/0'); open_branch_prompt(s)
            s.send(b'feature/picker-created\r')
            report = s.finish()
            assert 'session:new' in s.text(),s.text()[-2000:]
            new_tree=root/'picker trees/feature-picker-created'
            assert (new_tree/'.git').is_file()
            branch=subprocess.check_output(['git','-C',str(new_tree),'symbolic-ref','--short','HEAD'],text=True).strip()
            assert branch=='feature/picker-created'
            calls=created_calls()[before_create:]
            assert len(calls)==1,calls
            assert pathlib.Path(calls[0][calls[0].index('-c')+1]).resolve()==new_tree.resolve(),calls
            assert (repo/'picker-create-hooks').read_text()=='created'
            assert (repo/'picker-switch-hooks').read_text()=='switched'
            checks.append({'case':'worktree-create-from-empty-results-configured-path-hooks-once-and-connect',**report})
        finally:
            s.close()
        s = Session(binary, cfg, root, arguments=[*base, 'wt', 'pk', '--path', str(repo), '--query=feature/scoped'], env_extra=env)
        try:
            s.expect('feature/scoped'); s.send(b'\x18')
            s.expect('Remove this worktree using Worktrunk?')
            canceled_at = len(s.capture)
            s.send(b'n')
            s.until(lambda: b'Enter connect' in s.capture[canceled_at:], 'worktree picker did not reopen after cancel')
            assert tree.is_dir()
            confirm_at = len(s.capture)
            s.send(b'\x18')
            s.until(lambda: b'Confirm' in s.capture[confirm_at:], 'second removal confirmation missing')
            s.send(b'y')
            s.until(lambda: not tree.exists(), 'worktree confirmation did not remove checkout')
            s.expect('0/0'); s.send(b'\x1b')
            checks.append({'case':'worktree-confirm-cancel-then-safe-remove-and-refresh',**s.finish()})
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
            for source in (['--config'] if label == 'fzf-error' else ['--config', '--rex']):
                result = subprocess.run([*([binary]+base), 'picker', source], env=dict(os.environ, PATH=str(isolated)), capture_output=True, text=True, timeout=8)
                assert result.returncode == 1 and expected in result.stderr, (label, source, result)
                checks.append({'case': label + ('-rex' if source == '--rex' else ''), 'exit': result.returncode})
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
