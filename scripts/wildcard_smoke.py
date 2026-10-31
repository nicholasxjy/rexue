#!/usr/bin/env python3
"""Isolated CLI check of wildcard character classes."""
import argparse,json,os,pathlib,subprocess,tempfile


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/release/rexue');parser.add_argument('--expect-broken',action='store_true');args=parser.parse_args()
    binary=str(pathlib.Path(args.binary).resolve());records=[]
    with tempfile.TemporaryDirectory(prefix='rexue-wildcard-') as tmp:
        root=pathlib.Path(tmp);project=root/'project-r';project.mkdir();env=dict(os.environ,REXUE_STATE_DIR=str(root/'state'))
        for group,expected in [('^x','W'),('!x',None)]:
            pattern=str(project)[:-1]+'['+group+']';cfg=root/(group+'.toml')
            cfg.write_text('[[session]]\nname="probe"\npath='+json.dumps(str(project))+'\n[[wildcard]]\npattern='+json.dumps(pattern)+'\nicon="W"\n')
            result=subprocess.run([binary,'-C',str(cfg),'list','--config','--json'],capture_output=True,check=True,timeout=10,env=env)
            observed=json.loads(result.stdout)[0]['icon'];matches=observed==expected
            assert matches != args.expect_broken, (pattern,expected,observed)
            records.append({'pattern':pattern,'expected_icon':expected,'observed_icon':observed,'matches_pattern':matches})
    print(json.dumps({'wildcard_reproduction':records,'expected_broken_baseline':args.expect_broken},indent=2,ensure_ascii=False))


if __name__=='__main__':main()
