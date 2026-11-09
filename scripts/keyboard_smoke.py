#!/usr/bin/env python3
"""Exercise legacy and extended keyboard input through Rexue and real fzf."""
import argparse
import json
import os
import pathlib
import signal
import subprocess
import sys
import termios
import tempfile

from pty_smoke import Session, executable


CASES = {
    'legacy': (b'\x0a', b'\x0b'),
    'csi-u': (b'\x1b[106;5u', b'\x1b[107;5u'),
    'csi-u-text': (b'\x1b[106;5;106u', b'\x1b[107;5;107u'),
    'csi-u-repeat': (b'\x1b[106;5:2u', b'\x1b[107;5:2u'),
    'csi-u-alternate-keys': (b'\x1b[106:74:106;5u', b'\x1b[107:75:107;5u'),
    'modify-other-keys': (b'\x1b[27;5;106~', b'\x1b[27;5;107~'),
    'ctrl-n-p': (b'\x1b[110;5u', b'\x1b[112;5u'),
    'arrows': (b'\x1b[B', b'\x1b[A'),
    'suspend-resume': (b'\x1b[106;5u', b'\x1b[107;5u'),
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/rexue')
    parser.add_argument('--case', choices=CASES, action='append')
    parser.add_argument('--output', type=pathlib.Path)
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    checks = []
    with tempfile.TemporaryDirectory(prefix='rexue-keyboard-') as directory:
        root = pathlib.Path(directory)
        cfg = root / 'config.toml'
        rex = root / 'rex-fixture'
        executable(rex, '#!' + sys.executable + '\n' + '''import json,pathlib,sys
root=pathlib.Path(__file__).parent
a=sys.argv[4:]
if a[:1]==['-S']:a=a[2:]
if a[:2]==['do','--eval']:
    print('Error: unknown flag: --eval',file=sys.stderr);sys.exit(2)
if a[:1]==['ls']:
    print(json.dumps({'sessions':[{'session_id':'session:'+name,'label':name} for name in ['ALPHA','BETA','GAMMA']]}))
elif a[:2]==['session','inspect']:
    print(json.dumps({'active_window_id':'window:fixture','windows':[{'window_id':'window:fixture','label':'Terminal','focused_block_id':'block:fixture'}]}))
elif a[:2]==['block','call']:
    print(json.dumps({'foreground':{'cwd':str(root)}}))
elif a[:1]==['capture']:
    print('PREVIEW_'+a[2].split(':',1)[1])
else:
    print('{}')
''')
        config = ['strict_mode=true', '[tui]', 'show_tabs=true', 'preview=true', 'preview_min_width=20']
        for name in ('ALPHA', 'BETA', 'GAMMA'):
            config.extend(['[[session]]', f'name="{name}"',
                           'path=' + json.dumps(directory),
                           f'preview_command="printf PREVIEW_{name}"'])
        cfg.write_text('\n'.join(config) + '\n')
        for label in args.case or CASES:
            down, up = CASES[label]
            session = Session(binary, cfg, root, arguments=[
                '--rex-bin', str(rex), '--detach', '-C', str(cfg), 'picker'])
            try:
                session.expect('PREVIEW_ALPHA')
                if label == 'suspend-resume':
                    child = int(subprocess.check_output(
                        ['pgrep', '-P', str(session.proc.pid)], text=True).splitlines()[0])
                    session.send(b'\x1a')
                    session.until(lambda: bool(termios.tcgetattr(session.slave)[3] & termios.ICANON),
                                  'suspension did not restore canonical terminal input', timeout=2)
                    session.until(lambda: subprocess.check_output(
                        ['ps', '-o', 'state=', '-p', str(child)], text=True).strip().startswith('T'),
                        'picker process did not stop', timeout=2)
                    os.kill(child, signal.SIGCONT)
                    session.until(lambda: not (termios.tcgetattr(session.slave)[3] & termios.ICANON),
                                  'resume did not restore raw keyboard input', timeout=2)
                session.capture.clear()
                session.send(down * 2)
                session.expect('PREVIEW_BETA', timeout=2)
                session.capture.clear()
                session.send(up * 2)
                session.expect('PREVIEW_ALPHA', timeout=2)
                session.send(b'\x1b')
                checks.append({'case': label, 'passed': True, **session.finish()})
                print('PASS ' + label, flush=True)
            except Exception as error:
                message = str(error).split('\n', 1)[0]
                checks.append({'case': label, 'passed': False, 'error': message,
                               'terminal_tail': session.text()[-1500:]})
                print('FAIL ' + label + ': ' + message, flush=True)
            finally:
                session.close()
    receipt = json.dumps({'keyboard': checks}, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(receipt + '\n')
    print(receipt)
    return 0 if all(check['passed'] for check in checks) else 1


if __name__ == '__main__':
    sys.exit(main())
