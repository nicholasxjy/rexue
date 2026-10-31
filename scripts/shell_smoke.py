#!/usr/bin/env python3
"""Read-only reproduction of a nested shell path placeholder."""
import argparse,json,os,pathlib,subprocess,tempfile


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default='target/release/rexue');parser.add_argument('--expect-broken',action='store_true');args=parser.parse_args()
    binary=str(pathlib.Path(args.binary).resolve());project=pathlib.Path(__file__).resolve().parents[1]
    script='printf \'%s\' "$(basename \'{}\')"'
    with tempfile.TemporaryDirectory(prefix='rexue-shell-') as tmp:
        root=pathlib.Path(tmp);cfg=root/'shell.toml'
        cfg.write_text('[[session]]\nname="audit-preview"\npath='+json.dumps(str(project))+'\npreview_command='+json.dumps(script)+'\n')
        result=subprocess.run([binary,'-C',str(cfg),'preview','audit-preview'],capture_output=True,check=True,timeout=10,env=dict(os.environ,REXUE_STATE_DIR=str(root/'state')))
        observed=result.stdout.decode();matches=observed==project.name
        assert matches != args.expect_broken, (script,project.name,observed)
    print(json.dumps({'shell_placeholder_reproduction':{'script':script,'expected':project.name,'observed':observed,'matches_shell':matches},'expected_broken_baseline':args.expect_broken},ensure_ascii=False,indent=2))


if __name__=='__main__':main()
