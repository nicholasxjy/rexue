#!/usr/bin/env python3
"""Live Rex cleanup acceptance; creates and closes only test-owned sessions."""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/debug/rexue')
    parser.add_argument('--rex-bin', default='rex')
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    rex = shutil.which(args.rex_bin)
    wt = shutil.which('wt')
    assert rex and wt, 'Rex and Worktrunk must be installed'
    with tempfile.TemporaryDirectory(prefix='rexue-live-cleanup-') as tmp:
        root = pathlib.Path(tmp).resolve()
        env = dict(os.environ, REXUE_STATE_DIR=str(root/'state'), XDG_CONFIG_HOME=str(root/'xdg'),
                   XDG_CACHE_HOME=str(root/'cache'), GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1')
        config = root/'rexue.toml'
        config.write_text('strict_mode=true\n[frecency]\nlist_command="true"\nadd_command="true"\n')
        wt_config = root/'wt.toml'
        wt_config.write_text('')
        wrapper = root/'wt-wrapper'
        wrapper.write_text('#!' + sys.executable + '\nimport os,sys\nos.execv(' + repr(wt) + ',["wt","--config",' + repr(str(wt_config)) + ',*sys.argv[1:]])\n')
        wrapper.chmod(0o755)
        env['REXUE_WT'] = str(wrapper)

        def run(*command):
            result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, timeout=30)
            assert result.returncode == 0, (command, result.stderr)
            return result

        def rex_json(*command):
            return json.loads(run(rex, '--autostart=false', *command, '--json').stdout)

        def cli(*command):
            return json.loads(run(binary, '--rex-bin', rex, '--detach', '-C', str(config), *command, '--json').stdout)

        def wait_for(predicate, message):
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if predicate():
                    return
                time.sleep(0.1)
            raise AssertionError(message)

        def alive(pid):
            try:
                os.kill(pid, 0)
                return True
            except ProcessLookupError:
                return False

        repository = root/'repo'
        tree = root/'checkout'
        sibling = root/'checkout-sibling'
        sibling.mkdir()
        run('git', 'init', '--quiet', '-b', 'main', str(repository))
        run('git', '-C', str(repository), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
            '-c', 'commit.gpgsign=false', 'commit', '--quiet', '--allow-empty', '-m', 'fixture')
        run('git', '-C', str(repository), 'worktree', 'add', '-b', 'cleanup-test', str(tree))
        # A finite, inert process with an agent script name exercises real Rex
        # process discovery and teardown without starting an actual AI service.
        agent = root/'pi'
        agent.write_text('#!' + sys.executable + '\nimport os,pathlib,sys,time\npathlib.Path(sys.argv[1]).write_text(str(os.getpid()))\ntime.sleep(120)\n')
        agent.chmod(0o755)
        sessions = []
        pid_files = []

        def launch(command, directory):
            pid_file = root/f'process-{len(pid_files)}'
            result = run(rex, '--autostart=false', *command, '-c', str(directory), '--shell=none', '--json', '--', str(agent), str(pid_file))
            data = json.loads(result.stdout)
            if command[0] == 'new':
                sessions.append(data['session_id'])
            pid_files.append(pid_file)
            return data

        try:
            first = launch(['new', root.name + '-first'], tree)
            first_id = first['session_id']
            block = first['initial_windows'][0]['block_ids'][0]
            launch(['window', 'new', '-s', first_id, 'Inactive agents', '--focus=false'], tree)
            launch(['split', '-s', first_id, '-b', block, '--focus=false'], tree)
            second = launch(['new', root.name + '-second'], tree)
            unrelated = launch(['new', root.name + '-unrelated'], sibling)
            affected = {first_id, second['session_id']}
            wait_for(lambda: all(path.exists() for path in pid_files), 'test agent processes did not start')
            pids = [int(path.read_text()) for path in pid_files]
            wait_for(lambda: len([row for row in cli('list', '--agents') if row['session_id'] in affected]) == 4,
                     'Rex did not discover the four test agent panes')
            assert len(rex_json('session', 'inspect', first_id)['windows']) == 2
            assert all(alive(pid) for pid in pids)
            cli('worktree', 'remove', 'cleanup-test', '--path', str(repository))
            assert not tree.exists()
            remaining = {row['session_id'] for row in rex_json('ls')['sessions']}
            assert not affected & remaining
            assert unrelated['session_id'] in remaining
            assert not any(row['session_id'] in affected for row in cli('list', '--agents'))
            wait_for(lambda: all(not alive(pid) for pid in pids[:-1]), 'removed sessions left an agent process alive')
            assert alive(pids[-1]), 'unrelated session process was terminated'
            print(json.dumps({'passed': True, 'removed_sessions': 2, 'removed_tabs': 3,
                              'removed_agent_panes': 4, 'agent_processes_exited': 4,
                              'unrelated_session_preserved': True}, indent=2))
        finally:
            # Full IDs came only from this test's creation responses.
            remaining = {row['session_id'] for row in rex_json('ls')['sessions']}
            for session_id in reversed(sessions):
                if session_id in remaining:
                    run(rex, '--autostart=false', 'kill', session_id)


if __name__ == '__main__':
    main()
