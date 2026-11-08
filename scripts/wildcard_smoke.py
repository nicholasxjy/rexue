#!/usr/bin/env python3
"""Isolated CLI check of wildcard character classes."""
import argparse,json,os,pathlib,subprocess,sys,tempfile
from pty_smoke import executable


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/release/rexue');parser.add_argument('--expect-broken',action='store_true');args=parser.parse_args()
    binary=str(pathlib.Path(args.binary).resolve());records=[]
    with tempfile.TemporaryDirectory(prefix='rexue-wildcard-') as tmp:
        root=pathlib.Path(tmp);project=root/'project-r';project.mkdir();env=dict(os.environ,REXUE_STATE_DIR=str(root/'state'))
        rex=root/'rex-fixture'
        executable(rex,'#!'+sys.executable+'\n'+'''import json,pathlib,sys
a=sys.argv[4:]
if a[:1]==['-S']:a=a[2:]
if a[:2]==['do','--eval']:
    print('Error: unknown flag: --eval',file=sys.stderr);sys.exit(2)
if a[:1]==['ls']:
    print(json.dumps({'sessions':[{'session_id':'session:probe','label':'probe'}]}))
elif a[:2]==['session','inspect']:
    print(json.dumps({'active_window_id':'window:fixture','windows':[{'window_id':'window:fixture','focused_block_id':'block:fixture'}]}))
elif a[:2]==['block','call']:
    print(json.dumps({'foreground':{'cwd':str(pathlib.Path(__file__).parent/'project-r')}}))
else:
    print('{}')
''')
        for group,expected in [('^x','W'),('!x',None)]:
            pattern=str(project)[:-1]+'['+group+']';cfg=root/(group+'.toml')
            cfg.write_text('[[session]]\nname="probe"\npath='+json.dumps(str(project))+'\n[[wildcard]]\npattern='+json.dumps(pattern)+'\nicon="W"\n')
            result=subprocess.run([binary,'--rex-bin',str(rex),'-C',str(cfg),'list','--json'],capture_output=True,check=True,timeout=10,env=env)
            observed=json.loads(result.stdout)[0]['icon'];matches=observed==expected
            assert matches != args.expect_broken, (pattern,expected,observed)
            records.append({'pattern':pattern,'expected_icon':expected,'observed_icon':observed,'matches_pattern':matches})
    print(json.dumps({'wildcard_reproduction':records,'expected_broken_baseline':args.expect_broken},indent=2,ensure_ascii=False))


if __name__=='__main__':main()
