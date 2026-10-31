#!/usr/bin/env python3
"""Native TUI acceptance through a real Unix PTY; creates no Rex sessions."""
import argparse
import codecs
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
import unicodedata

MARKER = b'REXUE_TTY_REPORT='
PROJECT = '烟花-project'
OTHER = 'unrelated-notes'
STALE = 'STALE_A_PREVIEW'
FRESH = 'FRESH_B_PREVIEW'


def child():
    # Keep the controlling session alive until attributes have been checked: on
    # macOS the slave stops accepting tcgetattr after its session leader exits.
    initial = termios.tcgetattr(0)
    proc = subprocess.Popen(sys.argv[2:])
    signal.signal(signal.SIGTERM, lambda *_: proc.send_signal(signal.SIGTERM))
    code = proc.wait()
    report = {'restored': termios.tcgetattr(0) == initial, 'exit': code}
    os.write(1, MARKER + json.dumps(report).encode() + b'\n')
    return code


class Grid:
    """The VT subset emitted by crossterm/ratatui; unknown controls fail closed.

    Coordinates are zero-based. A wide glyph owns its next cell (empty string).
    Writing the rightmost cell sets pending wrap, not an out-of-bounds cursor.
    Styling and terminal modes do not change cells. Only alternate-screen writes
    are checked, so shell diagnostics after teardown cannot masquerade as UI.
    """
    def __init__(self, width=120, height=30):
        self.width, self.height = width, height
        self.cells = [[' '] * width for _ in range(height)]
        self.x = self.y = 0
        self.wrap = False
        self.active = False
        self.entered = self.left = False
        self.pending = ''
        self.decoder = codecs.getincrementaldecoder('utf-8')('strict')
        self.writes = 0
        self.clears = 0
        self.forbidden = None
        self.cursor_styles = []
        self.cursor_visible = True
        self.cursor_color_resets = 0
        self.reverse = False
        self.reverse_cells = [[False] * width for _ in range(height)]

    def resize(self, width, height):
        self.cells = [(row[:width] + [' '] * width)[:width]
                      for row in self.cells[:height]]
        self.cells += [[' '] * width for _ in range(height - len(self.cells))]
        self.reverse_cells=[(row[:width]+[False]*width)[:width] for row in self.reverse_cells[:height]]
        self.reverse_cells += [[False]*width for _ in range(height-len(self.reverse_cells))]
        self.width, self.height = width, height
        self.x, self.y = min(self.x, width - 1), min(self.y, height - 1)
        self.wrap = False

    def lines(self):
        return [''.join(row) for row in self.cells]

    def text(self):
        return '\n'.join(self.lines())

    def receipt(self):
        self.valid_cursor()
        return {'size': [self.width, self.height], 'cursor': [self.x, self.y],
                'screen': [line.rstrip() for line in self.lines()],
                'checked_cell_writes': self.writes, 'terminal_cursor_visible': self.cursor_visible,
                'cursor_styles': list(self.cursor_styles), 'cursor_color_resets': self.cursor_color_resets,
                'reversed_cells':[[x,y] for y in range(self.height) for x in range(self.width) if self.reverse_cells[y][x]]}

    def valid_cursor(self):
        assert 0 <= self.x < self.width and 0 <= self.y < self.height, (
            'cursor outside terminal', self.x, self.y, self.width, self.height)

    def csi(self, args, op):
        if op == 'q':
            assert args.endswith(' ') and args.strip().isdigit(), ('unsupported cursor style', args)
            style=int(args.strip());assert style in range(7), ('cursor style', style)
            self.cursor_styles.append(style);return
        if args.startswith('?'):
            assert op in 'hl', ('unsupported private CSI', args, op)
            modes = args[1:].split(';')
            assert set(modes) <= {'1049', '25', '2004', '7', '2026'}, modes
            if '25' in modes:self.cursor_visible=op=='h'
            if '1049' in modes:
                self.active = op == 'h'
                if self.active:
                    self.entered = True
                    self.cells = [[' '] * self.width for _ in range(self.height)]
                    self.reverse_cells = [[False]*self.width for _ in range(self.height)]
                    self.x = self.y = 0
                    self.wrap = False
                else:
                    self.left = True
            return
        if op == 'm':
            values=[int(v) if v else 0 for v in args.split(';')];i=0
            while i<len(values):
                code=values[i]
                if code in (38,48,58) and i+1<len(values):i+=3 if values[i+1]==5 else 5;continue
                if code==0:self.reverse=False
                elif code==7:self.reverse=True
                elif code==27:self.reverse=False
                i+=1
            return
        if not self.active:
            return
        values = [int(v) if v else 0 for v in args.split(';')]
        n = values[0] or 1
        if op in 'Hf':
            self.y, self.x = n - 1, (values[1] or 1) - 1 if len(values) > 1 else 0
        elif op == 'G':
            self.x = n - 1
        elif op == 'd':
            self.y = n - 1
        elif op == 'A':
            self.y -= n
        elif op == 'B':
            self.y += n
        elif op == 'C':
            self.x += n
        elif op == 'D':
            self.x -= n
        elif op == 'J':
            mode = values[0]
            assert mode in (0, 1, 2, 3), ('erase display', mode)
            if mode == 2:
                self.clears += 1
            for y in range(self.height):
                for x in range(self.width):
                    if mode in (2, 3) or (mode == 0 and (y, x) >= (self.y, self.x)) or (mode == 1 and (y, x) <= (self.y, self.x)):
                        self.cells[y][x] = ' '
                        self.reverse_cells[y][x] = self.reverse
        elif op == 'K':
            mode = values[0]
            assert mode in (0, 1, 2), ('erase line', mode)
            for x in range(self.width):
                if mode == 2 or (mode == 0 and x >= self.x) or (mode == 1 and x <= self.x):
                    self.cells[self.y][x] = ' '
                    self.reverse_cells[self.y][x] = self.reverse
        elif op != 'm':
            raise AssertionError(('unsupported CSI', args, op))
        if op != 'm':
            self.wrap = False
        self.valid_cursor()

    def put(self, char):
        if not self.active:
            return
        if char == '\r':
            self.x, self.wrap = 0, False
            return
        if char == '\n':
            self.y += 1
            self.wrap = False
            self.valid_cursor()
            return
        if char == '\b':
            self.x = max(0, self.x - 1)
            self.wrap = False
            return
        assert ord(char) >= 32 and char != '\x7f', ('unsupported control', repr(char))
        width = 0 if unicodedata.combining(char) or unicodedata.category(char) in ('Mn', 'Me', 'Cf') else (2 if unicodedata.east_asian_width(char) in ('W', 'F') else 1)
        if width == 0:
            x = self.x if self.wrap else self.x - 1
            while x > 0 and self.cells[self.y][x] == '':
                x -= 1
            assert x >= 0, 'combining character without base'
            self.cells[self.y][x] += char
            return
        if self.wrap:
            self.x, self.y, self.wrap = 0, self.y + 1, False
        self.valid_cursor()
        assert self.x + width <= self.width, ('glyph outside terminal', char, self.x, width)
        # Erase both halves if a diff overwrites part of a wide glyph.
        for x in range(self.x, self.x + width):
            if self.cells[self.y][x] == '' and x > 0:
                self.cells[self.y][x - 1] = ' '
            elif x + 1 < self.width and self.cells[self.y][x + 1] == '':
                self.cells[self.y][x + 1] = ' '
            self.cells[self.y][x] = ' '
        self.cells[self.y][self.x] = char
        for x in range(self.x,self.x+width):self.reverse_cells[self.y][x]=self.reverse
        if width == 2:
            self.cells[self.y][self.x + 1] = ''
        self.writes += width
        end = self.x + width
        self.x, self.wrap = min(end, self.width - 1), end == self.width
        if self.forbidden:
            assert self.forbidden not in ''.join(self.cells[self.y]), 'stale A preview rendered while B selected'

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith('\x1b['):
                match = re.match(r'\x1b\[([0-?]*[ -/]*)([@-~])', self.pending)
                if not match:
                    return
                self.csi(*match.groups())
                self.pending = self.pending[match.end():]
            elif self.pending.startswith('\x1b]'):
                endings=[(self.pending.find(end,2),end) for end in ['\x07','\x1b\\']]
                endings=[item for item in endings if item[0]>=0]
                if not endings:return
                stop,ending=min(endings,key=lambda item:item[0]);payload=self.pending[2:stop]
                assert payload=='112', ('unsupported OSC',payload)
                self.cursor_color_resets+=1;self.pending=self.pending[stop+len(ending):]
            elif self.pending.startswith('\x1b'):
                if len(self.pending) == 1:
                    return
                raise AssertionError(('unsupported escape', repr(self.pending[:40])))
            else:
                self.put(self.pending[0])
                self.pending = self.pending[1:]


class Session:
    def __init__(self, binary, cfg, root, source='-c', arguments=None, env_extra=None, virtual_cursor=False):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))

        def child_tty():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        env = dict(os.environ, TERM='xterm-256color', REXUE_STATE_DIR=str(root / 'state'))
        if env_extra:env.update(env_extra)
        self.virtual_cursor=virtual_cursor
        command = [binary, '--rex-bin', '/usr/bin/false', '--detach', '-C', str(cfg),
                   'picker', source, '--no-cache'] if arguments is None else [binary,*arguments]
        self.proc = subprocess.Popen(
            [sys.executable, str(pathlib.Path(__file__).resolve()), '--child', *command],
            stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env, cwd=root, preexec_fn=child_tty)
        self.capture = bytearray()
        self.grid = Grid()

    def drain(self, timeout=0.02):
        ready, _, _ = select.select([self.master], [], [], timeout)
        if ready:
            try:
                data = os.read(self.master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                return
            self.capture.extend(data)
            self.grid.feed(data)

    def until(self, predicate, message, timeout=3):
        deadline = time.monotonic() + timeout
        while not predicate():
            assert self.proc.poll() is None, (message, 'picker exited', bytes(self.capture[-1000:]))
            left = deadline - time.monotonic()
            assert left > 0, (message, self.grid.receipt())
            self.drain(min(0.01, left))
        self.grid.valid_cursor()

    def send(self, data):
        os.write(self.master, data)

    def resize(self, width, height):
        self.grid.resize(width, height)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
        os.killpg(self.proc.pid, signal.SIGWINCH)

    def finish(self, expected=0):
        self.until(lambda: MARKER in self.capture, 'terminal restoration report missing')
        self.proc.wait(timeout=1)
        self.drain(0.01)
        match = re.search(MARKER + rb'([^\r\n]+)', self.capture)
        assert match, bytes(self.capture[-1000:])
        report = json.loads(match[1])
        assert report == {'restored': True, 'exit': expected}, report
        assert self.proc.returncode == expected, self.proc.returncode
        assert self.grid.entered and self.grid.left, 'alternate screen lifecycle missing'
        assert self.grid.cursor_styles and self.grid.cursor_styles[-1]==0, ('cursor style not restored',self.grid.cursor_styles)
        if not self.virtual_cursor:assert 2 in self.grid.cursor_styles, ('session cursor not steady block',self.grid.cursor_styles)
        assert self.grid.cursor_visible, 'terminal cursor not restored visible'
        assert self.grid.cursor_color_resets>=2, ('cursor default color not restored',self.grid.cursor_color_resets)
        return {**report, 'bytes': len(self.capture),'cursor_styles_after_exit':list(self.grid.cursor_styles),'cursor_color_resets_after_exit':self.grid.cursor_color_resets,'cursor_visible_after_exit':self.grid.cursor_visible}

    def close(self):
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL)
            self.proc.wait()
        os.close(self.master)
        os.close(self.slave)


def long_name_case(binary,cfg,root):
    name='long-'+('x'*58)+'-TAIL'
    cfg.write_text('strict_mode=true\n[tui]\nshow_icons=false\npreview=false\n[[session]]\nname='+json.dumps(name)+'\npath='+json.dumps(str(root))+'\n')
    session=Session(binary,cfg,root)
    try:
        grid=session.grid;session.until(lambda:name in grid.text(),'list-only name was clipped at logical column 60');wide=grid.receipt()
        session.send(b'\x0f');session.until(lambda:split_visible(grid) and '-TAIL' not in grid.lines()[2][:48],'split preview did not clip name at divider');split=grid.receipt()
        clears=grid.clears;session.resize(80,12);session.until(lambda:grid.clears>clears and name in grid.text() and '│' not in grid.text(),'list-only name did not recover when preview hid');narrow=grid.receipt()
        session.send(b'\x1b');return {'test':'long-name-list-vs-preview','wide':wide,'split':split,'narrow':narrow,**session.finish()}
    finally:session.close()


def retained_preview_case(binary,cfg,root):
    fixture=root/'held-preview'
    fixture.write_text('#!'+sys.executable+'\n'+'''import pathlib,sys,time
root=pathlib.Path(__file__).parent
if sys.argv[1]=='A':print('HELD_A_PREVIEW',flush=True)
else:
    (root/'held-B.started').write_text('started')
    time.sleep(0.8)
    print('REPLACEMENT_B_PREVIEW',flush=True)
    (root/'held-B.finished').write_text('finished')
''')
    fixture.chmod(0o755);write_config(cfg,root,fixture);session=Session(binary,cfg,root)
    try:
        grid=session.grid;session.until(lambda:'HELD_A_PREVIEW' in grid.text(),'initial A preview did not appear');loaded=grid.receipt()
        session.send(b'\x1b[B');session.until(lambda:(root/'held-B.started').exists(),'B preview did not start');assert not (root/'held-B.finished').exists();assert 'HELD_A_PREVIEW' in grid.text(),'last displayed preview was cleared while B pending';pending=grid.receipt()
        session.until(lambda:'REPLACEMENT_B_PREVIEW' in grid.text() and (root/'held-B.finished').exists(),'B preview did not replace retained A');assert 'HELD_A_PREVIEW' not in grid.text();ready=grid.receipt()
        session.send(b'\x1b');return {'test':'retain-previous-preview-until-replacement','loaded':loaded,'pending':pending,'replacement':ready,**session.finish()}
    finally:session.close()

def confirmation_case(binary,cfg,root):
    target=root/'history-target';target.mkdir(exist_ok=True);marker=root/'removed-history'
    listing=root/'history-list';removal=root/'history-remove'
    listing.write_text('#!/bin/sh\nif [ ! -e '+shlex.quote(str(marker))+' ]; then printf \'%s\\n\' '+shlex.quote('1 history-target')+'; fi\n')
    removal.write_text('#!/bin/sh\nprintf \'%s\\n\' "$1" >> '+shlex.quote(str(marker))+'\n')
    listing.chmod(0o755);removal.chmod(0o755)
    cfg.write_text('strict_mode=true\n[tui]\npreview=false\n[frecency]\nlist_command='+json.dumps(shlex.quote(str(listing)))+'\nremove_command='+json.dumps(shlex.quote(str(removal))+' {}')+'\n')
    session=Session(binary,cfg,root,source='-z')
    try:
        grid=session.grid;session.until(lambda:'history-target' in grid.text(),'frecency fixture did not render')
        session.send(b'\x18');session.until(lambda:'Do you want to remove this directory from zoxide?' in grid.text() and 'history-target' in grid.text() and 'Yes' in grid.text() and 'No' in grid.text(),'initial confirmation did not open with the full reference prompt')
        normal=grid.receipt()
        clears=grid.clears;session.resize(24,5);session.until(lambda:grid.clears>clears and 'history-target' in grid.text() and 'Yes' in grid.text() and 'No' in grid.text(),'short confirmation did not show target and both choices')
        short=grid.receipt();assert not marker.exists()
        session.resize(24,2);session.until(lambda:'Resize terminal' in grid.text(),'tiny confirmation did not request resize')
        session.send(b'\r');session.until(lambda:'Confirmation disabled' in grid.text(),'tiny Enter was not visibly rejected')
        assert not marker.exists(),'tiny Enter invoked the removal callback';tiny=grid.receipt()
        session.resize(24,5);session.until(lambda:'history-target' in grid.text() and 'Yes' in grid.text() and 'No' in grid.text(),'confirmation did not recover after resize')
        recovered=grid.receipt();session.send(b'\r');session.until(lambda:marker.exists() and 'history-target' not in grid.text(),'visible confirmation did not remove the exact history entry')
        removed=marker.read_text().splitlines();assert len(removed)==1 and (pathlib.Path(removed[0]) if pathlib.Path(removed[0]).is_absolute() else root/removed[0]).resolve()==target.resolve(),removed
        session.send(b'\x1b');report=session.finish()
        return {'test':'short-confirmation-resize-guard','normal_screen':normal,'short_screen':short,'tiny_screen':tiny,'tiny_enter_called_removal':False,'recovered_screen':recovered,'removal_calls':len(removed),**report}
    finally:session.close()

def write_config(cfg, root, preview=None):
    text = 'strict_mode = true\n[tui]\nshow_icons = true\npreview = true\n'
    for name, alias in [(PROJECT, 'wp'), (OTHER, 'notes')]:
        text += '\n[[session]]\nname = ' + json.dumps(name, ensure_ascii=False)
        text += '\npath = ' + json.dumps(str(root)) + '\nalias = ' + json.dumps(alias) + '\n'
        if preview:
            command = shlex.quote(str(preview)) + (' A' if name == PROJECT else ' B')
            text += 'preview_command = ' + json.dumps(command) + '\n'
    cfg.write_text(text)


def split_visible(grid):
    # Default 60% preview at 120 columns: list 48, divider at column 48.
    return (grid.width, grid.height) == (120, 30) and all(row[48] == '│' for row in grid.cells)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/debug/rexue')
    parser.add_argument('--only-confirmation', action='store_true')
    args = parser.parse_args()
    binary = str(pathlib.Path(args.binary).resolve())
    results = []
    with tempfile.TemporaryDirectory(prefix='rexue-pty-') as directory:
        root = pathlib.Path(directory)
        cfg = root / 'config.toml'
        write_config(cfg, root)
        if args.only_confirmation:
            results.append(confirmation_case(binary,cfg,root))
            print(json.dumps({'pty':results},ensure_ascii=False,indent=2));return
        for test in ['escape', 'filter-resize', 'selected-error', 'sigterm']:
            session = Session(binary, cfg, root)
            try:
                grid = session.grid
                session.until(lambda: PROJECT in grid.text() and OTHER in grid.text() and split_visible(grid),
                              'initial two-entry list and 120x30 preview split did not render')
                receipt = {'test': test}
                if test == 'filter-resize':
                    receipt['initial'] = grid.receipt()
                    session.send(b'project')
                    session.until(lambda: '> project' in grid.lines()[0] and PROJECT in grid.text() and OTHER not in grid.text(),
                                  'filter did not remove the nonmatching entry')
                    assert split_visible(grid), 'wide preview divider missing after filtering'
                    receipt['filtered_wide'] = grid.receipt()
                    writes_before, clears_before = grid.writes, grid.clears
                    session.resize(42, 12)
                    # Require the backend's resize clear followed by real text:
                    # clipping the old wide screen alone must never pass.
                    session.until(lambda: grid.clears > clears_before and grid.writes > writes_before and '> project' in grid.lines()[0] and PROJECT in grid.text() and OTHER not in grid.text() and '│' not in grid.text(),
                                  '42x12 repaint did not hide preview and retain filtered entry')
                    receipt['filtered_narrow'] = grid.receipt()
                    session.send(b'\x1b')
                elif test == 'sigterm':
                    session.proc.send_signal(signal.SIGTERM)
                else:
                    session.send(b'\r' if test == 'selected-error' else b'\x1b')
                receipt.update(session.finish(1 if test == 'selected-error' else 0))
                results.append(receipt)
            finally:
                session.close()

        results.append(confirmation_case(binary,cfg,root))

        results.append(long_name_case(binary,cfg,root))
        results.append(retained_preview_case(binary,cfg,root))

        # An executable preview command, not a mocked worker. Markers carry the
        # same monotonic clock as the driver and prove input was handled mid-job.
        fixture = root / 'slow-preview'
        fixture.write_text('#!' + sys.executable + '\n' + '''import json, pathlib, sys, time
root = pathlib.Path(__file__).parent
name = sys.argv[1]
def mark(phase):
    (root / (name + '.' + phase)).write_text(json.dumps({'at': time.monotonic()}))
mark('started')
time.sleep(1)
print('STALE_A_PREVIEW' if name == 'A' else 'FRESH_B_PREVIEW', flush=True)
mark('finished')
''')
        fixture.chmod(0o755)
        write_config(cfg, root, fixture)
        session = Session(binary, cfg, root)
        try:
            grid = session.grid
            session.until(lambda: PROJECT in grid.text() and OTHER in grid.text() and split_visible(grid) and (root / 'A.started').exists(),
                          'slow A preview did not start')
            assert not (root / 'A.finished').exists(), 'A finished before input could be tested'
            grid.forbidden = STALE
            sent = time.monotonic()
            session.send(b'notes')
            session.until(lambda: '> notes' in grid.lines()[0] and OTHER in grid.text() and PROJECT not in grid.text(),
                          'filter blocked behind slow A preview', timeout=0.5)
            filtered = time.monotonic()
            assert filtered - sent <= 0.5, 'filter exceeded 500ms responsiveness bound'
            assert not (root / 'A.finished').exists(), 'A was no longer running when filter rendered'
            responsive_screen = grid.receipt()
            session.until(lambda: FRESH in grid.text() and (root / 'B.finished').exists(),
                          'B preview never appeared after stale A completed', timeout=4)
            # Observe beyond B completion as well; the per-character guard has
            # rejected stale A at every rendered write since switching to B.
            until = time.monotonic() + 0.15
            while time.monotonic() < until:
                session.drain(0.01)
            markers = {name: json.loads((root / name).read_text())['at']
                       for name in ['A.started', 'A.finished', 'B.started', 'B.finished']}
            assert markers['A.started'] <= sent < filtered < markers['A.finished'] <= markers['B.started'] < markers['B.finished'], markers
            assert FRESH in grid.text() and STALE not in grid.text()
            receipt = {'test': 'slow-preview', 'filter_ms': round((filtered - sent) * 1000, 2),
                       'a_running_at_filtered_screen': True, 'stale_a_rendered': False,
                       'markers_ms_from_input': {key: round((at - sent) * 1000, 2) for key, at in markers.items()},
                       'responsive_screen': responsive_screen, 'final_screen': grid.receipt()}
            session.send(b'\x1b')
            receipt.update(session.finish())
            results.append(receipt)
        finally:
            session.close()
    print(json.dumps({'pty': results}, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    sys.exit(child() if len(sys.argv) > 1 and sys.argv[1] == '--child' else main())
