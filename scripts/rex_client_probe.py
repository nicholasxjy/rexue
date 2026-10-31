#!/usr/bin/env python3
"""Observe Rex.app's focused pane through its official copy-ID action.
The clipboard is saved as bytes and restored; its previous content is never logged.
"""
import argparse,subprocess,time,uuid

def focused_block(client,rex='rex'):
    def paste():return subprocess.run(['pbpaste'],capture_output=True,check=True,timeout=5).stdout
    def copy(value):subprocess.run(['pbcopy'],input=value,check=True,timeout=5)
    previous=paste();sentinel=('rexue-probe-'+uuid.uuid4().hex).encode()
    try:
        copy(sentinel)
        subprocess.run([rex,'-C',client,'do','pane.copy_id','--json'],check=True,capture_output=True,timeout=15)
        deadline=time.monotonic()+2
        while time.monotonic()<deadline:
            value=paste().decode('utf-8',errors='replace').strip()
            if value.startswith('block:') and len(value)>10:return value
            time.sleep(0.04)
        raise AssertionError('Rex app copy-ID action did not expose a focused pane ID')
    finally:copy(previous)

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--client',required=True);p.add_argument('--rex',default='rex');a=p.parse_args()
    print(focused_block(a.client,a.rex))
