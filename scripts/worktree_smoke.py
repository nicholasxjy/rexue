#!/usr/bin/env python3
"""Offline acceptance: real wt/Git, isolated configs/hooks and a stateful Rex fake."""
import argparse
import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/rexue')
    parser.add_argument('--output', type=pathlib.Path)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    real_wt = shutil.which('wt')
    assert real_wt, 'Worktrunk >=0.81.0 is required for this acceptance script'
    checks = []
    with tempfile.TemporaryDirectory(prefix="rexue wt ' spaces ") as tmp:
        root = pathlib.Path(tmp).resolve()
        repo = root / 'repository'
        repo.mkdir()
        env = dict(os.environ, XDG_CONFIG_HOME=str(root / 'xdg'), XDG_CACHE_HOME=str(root / 'cache'),
                   REXUE_STATE_DIR=str(root / 'state'), GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1')
        config = root / 'rexue.toml'
        config.write_text('strict_mode=true\n[frecency]\nlist_command="true"\nquery_command="false"\nadd_command="true"\nremove_command="true"\n[[worktree]]\npath=' + json.dumps(str(repo)) + '\nicon="W"\n')
        wt_config = root / 'wt.toml'
        wt_config.write_text('worktree-path="../custom trees/{{ branch | sanitize }}"\n')
        wrapper = root / 'wt wrapper'
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(real_wt) + ' --config ' + shlex.quote(str(wt_config)) + ' "$@"\n')
        wrapper.chmod(0o755)
        env['REXUE_WT'] = str(wrapper)
        fake = root / 'fake rex'
        fake.write_text('#!' + sys.executable + '\n' + '''import json,pathlib,sys
root=pathlib.Path(__file__).parent
args=sys.argv[1:]
while args and args[0].startswith('-'):
 if args[0] in ('--timeout','-S'):args=args[2:]
 else:args=args[1:]
if args[:2]==['do','--eval']:
 print('Error: unknown flag: --eval',file=sys.stderr);sys.exit(2)
with (root/'rex-calls.jsonl').open('a') as f:f.write(json.dumps(args)+'\\n')
state=root/'rex-sessions.json'
sessions=json.loads(state.read_text()) if state.exists() else []
if args[:2]==['ls','--json']: print(json.dumps({'sessions':sessions}))
elif args[:2]==['whoami','--json']: print('{}')
elif args[:2]==['session','inspect']:
 s=next(s for s in sessions if s['session_id']==args[2]);print(json.dumps({'session':s,'windows':[]}))
elif args[0]=='new':
 counter=root/'next-session-id';number=int(counter.read_text())+1 if counter.exists() else 1;counter.write_text(str(number))
 s={'session_id':'session:'+str(number),'label':args[1],'path':args[args.index('-c')+1]};sessions.append(s);state.write_text(json.dumps(sessions));print(json.dumps({'session_id':s['session_id'],'initial_windows':[{'block_ids':['block:fixture']}]}))
elif args[0]=='kill':
 state.write_text(json.dumps([s for s in sessions if s['session_id']!=args[1]]));print('{}')
else: print('{}')
''')
        fake.chmod(0o755)
        base = [binary, '--rex-bin', str(fake), '--detach', '-C', str(config)]

        def run(command, cwd=repo, expected=0, input=None, local_env=env):
            result = subprocess.run(command, cwd=cwd, env=local_env, input=input, text=True, capture_output=True, timeout=45)
            if expected == 0:
                assert result.returncode == 0, (command, result.stdout, result.stderr)
            else:
                assert result.returncode != 0, (command, result.stdout, result.stderr)
            return result

        def cli(*args, **kwargs):
            return run([*base, *args], **kwargs)

        def wt(*args, **kwargs):
            return run([str(wrapper), *args], **kwargs)

        def git(*args):
            return run(['git', '-C', str(repo), *args]).stdout.strip()

        git('init', '-b', 'main')
        (repo / '.config').mkdir()
        (repo / '.config/wt.toml').write_text('pre-start = "printf started >> {{ repo_path }}/created-hooks"\npre-remove = "printf removed >> {{ repo_path }}/removed-hooks"\n')
        git('add', '.config/wt.toml')
        git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-m', 'fixture')
        # The first create must preserve native approval in rexue's own subprocess.
        # Approval state is task-private and applies only to these fixture commands.
        from pty_smoke import Session
        approval = Session(binary, config, root, arguments=[*base[1:], 'worktree', 'create', 'feature/widgets', '--path', str(repo), '--json'], env_extra=env)
        try:
            approval.expect('Allow')
            approval.send(b'y\r')
            approval.finish()
            match = re.search(r'\{\s*"path":\s*"(?:[^"\\]|\\.)*"\s*,\s*"created":\s*true\s*\}', approval.text())
            assert match, approval.text()
            first = json.loads(match[0])
        finally:
            approval.close()
        checks.append('native-hook-approval-in-terminal')
        tree = pathlib.Path(first['path'])
        assert first['created'] and tree == root / 'custom trees/feature-widgets', first
        assert (repo / 'created-hooks').read_text() == 'started'
        assert not (root / 'rex-sessions.json').exists(), 'create must not connect Rex'
        checks.extend(['configured-path-template', 'approved-project-pre-start-hook', 'create-without-rex'])
        rows = json.loads(cli('worktree', 'list', '--json', '--path', str(tree)).stdout)
        assert {r['branch'] for r in rows} == {'main', 'feature/widgets'}
        assert all(r['icon'] == 'W' for r in rows)
        checks.append('discover-from-linked-checkout-and-icons')
        connected = json.loads(cli('worktree', 'connect', 'feature/widgets', '--json').stdout)
        assert pathlib.Path(connected['path']) == tree and connected['created']
        again = json.loads(cli('worktree', 'connect', '--path', str(tree), '--json').stdout)
        assert not again['created'] and again['session_id'] == connected['session_id']
        assert (repo / 'created-hooks').read_text() == 'started', 'reuse reran create hook'
        calls = [json.loads(line) for line in (root / 'rex-calls.jsonl').read_text().splitlines()]
        creation = next(call for call in calls if call[0] == 'new')
        assert pathlib.Path(creation[creation.index('-c')+1]) == tree
        checks.extend(['rex-actual-cwd', 'rex-session-reuse', 'no-create-hook-on-reuse'])
        global_rows = json.loads(cli('list', '--json').stdout)
        assert any(r['source'] == 'worktree' and pathlib.Path(r['path']) == tree for r in global_rows)
        checks.append('global-worktree-source')
        wt_config.write_text(wt_config.read_text()+'\n[list]\njson-schema=1\n')
        schema1 = json.loads(cli('worktree', 'list', '--json').stdout)
        assert sorted(schema1, key=lambda row: row['path']) == sorted(rows, key=lambda row: row['path']), (schema1, rows)
        checks.append('list-schema-1-and-2')
        # Existing unnumbered checkouts at arbitrary paths must remain discoverable.
        arbitrary = root / 'legacy checkout'
        git('worktree', 'add', '--detach', str(arbitrary), 'HEAD')
        assert any(pathlib.Path(r['path']) == arbitrary and r['branch'] is None for r in json.loads(cli('worktree', 'list', '--json').stdout))
        checks.append('existing-arbitrary-detached-checkout')
        approval = Session(str(wrapper), config, root, arguments=['-C', str(repo), 'config', 'approvals', 'add'], env_extra=env)
        try:
            approval.expect('Allow')
            approval.send(b'y\r')
            approval.finish()
        finally:
            approval.close()
        (tree / 'dirty file').write_text('unsaved')
        failure = cli('worktree', 'remove', 'feature/widgets', expected=1)
        assert tree.is_dir() and (tree / 'dirty file').read_text() == 'unsaved'
        assert 'uncommitted' in failure.stderr.lower() or 'dirty' in failure.stderr.lower(), failure.stderr
        assert not any(call[0] == 'kill' for call in [json.loads(line) for line in (root / 'rex-calls.jsonl').read_text().splitlines()])
        (tree / 'dirty file').unlink()
        result = json.loads(cli('worktree', 'remove', str(tree), '--no-delete-branch', '--json').stdout)
        assert not tree.exists() and git('show-ref', '--verify', 'refs/heads/feature/widgets')
        assert result[0]['branch_outcome'] == 'not_attempted'
        assert (repo / 'removed-hooks').read_text().endswith('removed')
        kills = [call for call in [json.loads(line) for line in (root / 'rex-calls.jsonl').read_text().splitlines()] if call[0] == 'kill']
        assert kills == [['kill', connected['session_id']]], kills
        assert not json.loads((root / 'rex-sessions.json').read_text())
        checks.extend(['dirty-checkout-protection', 'foreground-remove', 'keep-branch', 'approved-pre-remove-hook', 'close-rex-session-by-recorded-path'])
        error = cli('worktree', 'remove', 'feature/widgets', expected=1)
        assert 'not a registered worktree' in error.stderr
        wt_config.write_text(wt_config.read_text().replace('custom trees', 'new layout'))
        recreated = json.loads(cli('worktree', 'connect', 'feature/widgets', '--name', 'moved-widgets', '--json').stdout)
        assert recreated['session_id'] != connected['session_id']
        assert pathlib.Path(recreated['path']) == root / 'new layout/feature-widgets'
        checks.append('live-configuration-change')
        # A terminal can enter a worktree while retaining the repository label.
        # Switching back to main must create/reuse a distinct name automatically.
        session_state = root / 'rex-sessions.json'
        sessions = json.loads(session_state.read_text())
        sessions[-1]['label'] = 'repository'
        retained_id = sessions[-1]['session_id']
        session_state.write_text(json.dumps(sessions))
        main = json.loads(cli('worktree', 'connect', 'main', '--json').stdout)
        assert main['created'] and main['name'] == 'repository@main', main
        assert pathlib.Path(main['path']) == repo and main['session_id'] != retained_id
        main_again = json.loads(cli('worktree', 'connect', 'main', '--json').stdout)
        assert not main_again['created'] and main_again['session_id'] == main['session_id']
        retained = next(s for s in json.loads(session_state.read_text()) if s['session_id'] == retained_id)
        assert retained['label'] == 'repository' and pathlib.Path(retained['path']) == root / 'new layout/feature-widgets'
        checks.append('worktree-switch-auto-name-collision-and-reuse')
        for version in [None, '0.80.0']:
            if version is None:
                executable = root / 'missing-wt'
            else:
                executable = root / 'old-wt'; executable.write_text('#!/bin/sh\necho wt v'+version+'\n'); executable.chmod(0o755)
            failure = cli('worktree', 'list', expected=1, local_env=dict(env, REXUE_WT=str(executable)))
            assert '0.81.0' in failure.stderr
        checks.extend(['missing-wt-diagnostic', 'incompatible-wt-diagnostic'])
        fail = root / 'failing-wt'
        fail.write_text('#!/bin/sh\nif [ "$1" = --version ]; then echo wt v0.81.0; else echo fixture-failure >&2; exit 7; fi\n'); fail.chmod(0o755)
        assert 'fixture-failure' in cli('worktree', 'list', expected=1, local_env=dict(env, REXUE_WT=str(fail))).stderr
        checks.append('subprocess-error-propagation')
    receipt = json.dumps({'checks': checks, 'count': len(checks), 'worktrunk': subprocess.check_output([real_wt, '--version'], text=True).strip()}, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(receipt + '\n')
    print(receipt)


if __name__ == '__main__':
    main()
