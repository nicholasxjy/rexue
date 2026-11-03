#!/usr/bin/env python3
"""Current-worktree removal: real fzf/Git/Worktrunk, isolated Rex fixture."""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

from pty_smoke import Session, executable


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/rexue')
    parser.add_argument('--output', type=pathlib.Path)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    real_wt = shutil.which('wt')
    assert real_wt, 'Worktrunk >=0.81.0 is required'
    checks = []
    with tempfile.TemporaryDirectory(prefix="rexue remove ' ; ") as directory:
        root = pathlib.Path(directory).resolve()
        config = root/'rexue.toml'
        wt_config = root/'wt.toml'
        wt_config.write_text('')
        wrapper = root/'wt-wrapper'
        executable(wrapper, '#!' + sys.executable + '\nimport os,sys\nos.execv(' + repr(real_wt) + ',["wt","--config",' + repr(str(wt_config)) + ',*sys.argv[1:]])\n')
        rex = root/'rex-fixture'
        executable(rex, '#!' + sys.executable + '\n' + '''import json,pathlib,sys
root=pathlib.Path(__file__).parent
args=sys.argv[4:]
if args[:1]==['-S']:args=args[2:]
with (root/'calls.jsonl').open('a') as f:f.write(json.dumps(args)+'\\n')
sessions=json.loads((root/'sessions.json').read_text())
current=json.loads((root/'current.json').read_text())
if args[:2]==['whoami','--json']:print(json.dumps({'session_id':current}))
elif args[:2]==['ls','--json']:print(json.dumps({'sessions':sessions}))
elif args[:2]==['session','inspect']:
    print(json.dumps({'active_window_id':'window:fixture','windows':[{'window_id':'window:fixture','focused_block_id':'block:fixture'}]}))
elif args[:2]==['block','call']:
    session=next(s for s in sessions if s['session_id']==args[3])
    print(json.dumps({'foreground':{'cwd':session['path']}}))
elif args[:2]==['client','ls']:
    print(json.dumps({'clients':[{'client_id':'client:fixture-full','info':{'kind':'app','label':'Fixture','platform':'macos'},'connection_state':'connected'}]}))
elif args[:1]==['-C']:
    if args[2]=='do' and (root/'switch-fail').exists():
        print('fixture switch failure',file=sys.stderr);sys.exit(7)
    print(json.dumps({'actions':[{'name':'session.select'}]} if args[2]=='actions' else {}))
elif args[:1]==['new']:
    session={'session_id':'session:created-full','label':args[1],'path':args[args.index('-c')+1]}
    (root/'sessions.json').write_text(json.dumps(sessions+[session]))
    print(json.dumps({'session_id':session['session_id'],'initial_windows':[{'block_ids':['block:fixture']}]}))
elif args[:1]==['kill']:
    (root/'sessions.json').write_text(json.dumps([s for s in sessions if s['session_id']!=args[1]]))
    print('{}')
else:
    print('unexpected fixture call: '+repr(args),file=sys.stderr);sys.exit(91)
''')
        env = dict(os.environ, REXUE_WT=str(wrapper), REX_CLIENT='client:fixture-full', XDG_CONFIG_HOME=str(root/'xdg'),
                   XDG_CACHE_HOME=str(root/'cache'), REXUE_STATE_DIR=str(root/'state'),
                   GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1')

        def run(*command, cwd=None):
            result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, (command, result.stdout, result.stderr)
            return result.stdout

        seed = root/'seed'
        run('git', 'init', '--quiet', '-b', 'main', str(seed))
        run('git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false',
            'commit', '--quiet', '--allow-empty', '-m', 'fixture', cwd=seed)
        for case, global_picker, empty_worktrees, other_sessions in [
            ('current-worktree-first-row', False, False, True),
            ('global-current-worktree-first-row', True, False, True),
            ('current-worktree-creates-first-session', False, False, False),
            ('current-worktree-switch-fails', False, False, True),
            ('noncurrent-worktree-keeps-sessions', False, False, True),
            ('last-worktree-first-session', False, True, True),
            ('last-worktree-no-other-sessions', False, True, False),
        ]:
            print('PTY: ' + case, flush=True)
            repository = root/(case + '-repo')
            run('git', 'clone', '--quiet', *(['--bare'] if empty_worktrees else []), str(seed), str(repository))
            current_tree = root/(case + '-tree')
            branch = 'feature/' + case
            run('git', 'worktree', 'add', '-b', branch, str(current_tree), 'main', cwd=repository)
            target_path = root
            if not empty_worktrees:
                target_path = root/(case + '-first')
                run('git', 'worktree', 'add', '-b', 'aaa-first', str(target_path), cwd=repository)
            nested = current_tree/'nested'
            nested.mkdir()
            current_id, target_id = 'session:current-full', 'session:first-full'
            sessions = [{'session_id': current_id, 'label': 'duplicate', 'path': str(nested)}]
            if other_sessions:
                sessions.extend([
                    {'session_id': target_id, 'label': 'duplicate', 'path': str(target_path)},
                    {'session_id': 'session:other-full', 'label': 'other', 'path': str(root)},
                ])
            if case == 'noncurrent-worktree-keeps-sessions':
                sessions[0]['path'] = str(target_path)
            if case == 'current-worktree-switch-fails':
                (root/'switch-fail').touch()
            (root/'sessions.json').write_text(json.dumps(sessions))
            (root/'current.json').write_text(json.dumps(current_id))
            (root/'calls.jsonl').write_text('')
            config.write_text('strict_mode=true\n[frecency]\nlist_command="true"\nadd_command="true"\n[tui]\npreview=false\n[[worktree]]\npath=' + json.dumps(str(repository)) + '\n')
            picker = ['picker'] if global_picker else ['worktree', 'picker', '--path', str(current_tree)]
            s = Session(binary, config, root, arguments=['--rex-bin', str(rex), '-C', str(config), *picker, '--query=' + branch], env_extra=env, cwd=nested)
            try:
                s.expect(branch)
                s.send(b'\x18')
                s.expect('Remove this worktree using Worktrunk?')
                if case == 'current-worktree-first-row':
                    offset = len(s.capture)
                    s.send(b'n')
                    s.until(lambda: b'Enter connect' in s.capture[offset:], 'cancel did not reopen worktree picker')
                    assert current_tree.is_dir()
                    dirty = current_tree/'dirty'
                    dirty.write_text('preserve me')
                    offset = len(s.capture)
                    s.send(b'\x18')
                    s.until(lambda: b'Confirm >' in s.capture[offset:], 'dirty removal confirmation missing')
                    s.send(b'y')
                    s.expect('Worktrunk remove failed')
                    calls = [json.loads(line) for line in (root/'calls.jsonl').read_text().splitlines()]
                    assert dirty.read_text() == 'preserve me'
                    assert not any(call[0] == 'kill' or 'session.select' in call for call in calls)
                    dirty.unlink()
                    offset = len(s.capture)
                    s.send(b'\x18')
                    s.until(lambda: b'Confirm >' in s.capture[offset:], 'final removal confirmation missing')
                s.send(b'y')
                if case == 'noncurrent-worktree-keeps-sessions':
                    s.until(lambda: not current_tree.exists(), 'noncurrent worktree was not removed')
                    s.expect('0/0')
                    s.send(b'\x1b')
                report = s.finish(expected=1 if case == 'current-worktree-switch-fails' else 0)
                assert not current_tree.exists()
                calls = [json.loads(line) for line in (root/'calls.jsonl').read_text().splitlines()]
                kills = [call for call in calls if call[0] == 'kill']
                switches = [call for call in calls if call[0] == '-C' and 'session.select' in call]
                if case in ['current-worktree-switch-fails', 'noncurrent-worktree-keeps-sessions']:
                    assert kills == [], kills
                    assert any(s['session_id'] == current_id for s in json.loads((root/'sessions.json').read_text()))
                    if case == 'current-worktree-switch-fails':
                        assert len(switches) == 1 and 'fixture switch failure' in s.text()
                    else:
                        assert switches == [], switches
                else:
                    assert kills == [['kill', current_id]], kills
                    if other_sessions or not empty_worktrees:
                        expected_id = target_id if other_sessions else 'session:created-full'
                        assert len(switches) == 1 and json.loads(switches[0][switches[0].index('--args')+1])['session_id'] == expected_id, switches
                        assert calls.index(switches[0]) < calls.index(kills[0]), calls
                    else:
                        assert switches == [], switches
                checks.append({'case': case, **report})
            except AssertionError:
                transcript = [line.strip() for line in s.text().splitlines() if line.strip()]
                print(json.dumps({'transcript': transcript[-35:], 'rex_calls': (root/'calls.jsonl').read_text().splitlines()[-20:]}, ensure_ascii=False, indent=2))
                raise
            finally:
                s.close()
                (root/'switch-fail').unlink(missing_ok=True)
    receipt = json.dumps({'checks': checks, 'count': len(checks)}, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(receipt + '\n')
    print(receipt)


if __name__ == '__main__':
    main()
