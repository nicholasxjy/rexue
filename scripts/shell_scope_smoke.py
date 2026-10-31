#!/usr/bin/env python3
"""Read-only reproduction of path binding across shell positional scopes."""
import argparse,json,os,pathlib,subprocess,tempfile


def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',default='target/release/rexue');p.add_argument('--expect-broken',action='store_true');a=p.parse_args()
    binary=str(pathlib.Path(a.binary).resolve());project=pathlib.Path(__file__).resolve().parents[1];records=[]
    with tempfile.TemporaryDirectory(prefix='rexue-shell-scope-') as tmp:
        root=pathlib.Path(tmp);env=dict(os.environ,REXUE_STATE_DIR=str(root/'state'))
        for script in ["f() { printf '%s' '{}'; }; f other","set -- other; printf '%s' '{}'"]:
            cfg=root/'scope.toml';cfg.write_text('[[session]]\nname="audit-preview"\npath='+json.dumps(str(project))+'\npreview_command='+json.dumps(script)+'\n')
            result=subprocess.run([binary,'-C',str(cfg),'preview','audit-preview'],capture_output=True,check=True,timeout=10,env=env)
            observed=result.stdout.decode();matches=observed==str(project)
            assert matches != a.expect_broken, (script,str(project),observed)
            records.append({'script':script,'expected':str(project),'observed':observed,'matches_project_path':matches})
    print(json.dumps({'shell_scope_reproduction':records,'expected_broken_baseline':a.expect_broken},ensure_ascii=False,indent=2))


if __name__=='__main__':main()
