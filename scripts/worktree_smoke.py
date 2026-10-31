#!/usr/bin/env python3
"""Read-only GitHub metadata + local Git worktree + isolated Rex integration."""
import argparse,json,os,pathlib,subprocess,tempfile
from pty_smoke import Session

def worktree_cursor_probe(binary,cfg,root,env):
    # --refresh forces the one initial fetch and disables a follow-up worker,
    # so the private repository/cache cannot be recreated after cleanup.
    arguments=['--rex-bin','/usr/bin/false','--detach','-C',str(cfg),'worktree','picker','--repo','joshmedeski/sesh','--refresh']
    session=Session(binary,cfg,root,arguments=arguments,env_extra=env,virtual_cursor=True)
    try:
        grid=session.grid;session.until(lambda:'409' in grid.text() and grid.reverse_cells[0][4] and not grid.cursor_visible,'worktree virtual cursor did not appear',timeout=8);visible=grid.receipt()
        session.until(lambda:not grid.reverse_cells[0][4] and not grid.cursor_visible,'worktree virtual cursor did not blink off');hidden=grid.receipt()
        query='zzzero-match-734129';session.send(query.encode());session.until(lambda:query in grid.lines()[0] and '409' not in '\n'.join(grid.lines()[2:]),'worktree unmatched filter did not repaint');assert 'No worktrees found' not in grid.text(),'unmatched filter was misreported as an empty catalog';unmatched=grid.receipt()
        session.send(b'\x1b');report=session.finish()
        return {'virtual_cursor_visible':visible,'virtual_cursor_hidden':hidden,'unmatched_filter':unmatched,**report}
    finally:session.close()

def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',default='target/debug/rexue');a=p.parse_args();binary=str(pathlib.Path(a.binary).resolve())
    def run(args,cwd=None,env=None):
        r=subprocess.run(args,cwd=cwd,env=env,text=True,capture_output=True,timeout=45)
        if r.returncode:raise RuntimeError(f'{args} failed: {r.stderr}')
        return r.stdout
    sessions=[]
    with tempfile.TemporaryDirectory(prefix='rexue-gh-live-') as directory:
        root=pathlib.Path(directory);repo=root/'repo';repo.mkdir();cfg=root/'config.toml'
        run(['git','init','-b','main',str(repo)]);(repo/'seed').write_text('seed\n')
        run(['git','-C',str(repo),'add','seed']);run(['git','-C',str(repo),'-c','user.name=Rexue Test','-c','user.email=rexue-test@example.invalid','-c','commit.gpgsign=false','commit','-m','test fixture'])
        cfg.write_text('strict_mode = true\n[frecency]\nlist_command="true"\nquery_command="false"\nadd_command="true"\nremove_command="true"\n[[worktree]]\nrepo="joshmedeski/sesh"\npath='+json.dumps(str(repo))+'\nfetch=false\nbase_branch="main"\ncreate_command="printf created > {}/created-marker"\nstartup_command="printf startup > {}/startup-marker"\nicon="🌿"\n')
        env=dict(os.environ,REXUE_STATE_DIR=str(root/'state'),XDG_CACHE_HOME=str(root/'cache'))
        base=[binary,'--detach','-C',str(cfg)]
        def bar(*args):return run([*base,*args],cwd=repo,env=env)
        try:
            clone=json.loads(run([*base,"connect",str(repo),"--json"],cwd=repo,env=env));sessions.append(clone["session_id"])
            cloned_id=bar("clone",str(repo),"-c",str(root),"-d","clone-copy").strip();sessions.append(cloned_id)
            assert (root/"clone-copy"/"seed").read_text()=="seed\n"
            assert bar("root",cloned_id).strip()=="clone-copy"
            plain=root/'plain-root-scope';plain.mkdir();plain_session=json.loads(bar('connect','--root',str(plain),'--json'));sessions.append(plain_session['session_id'])
            plain_again=json.loads(bar('connect','--root',str(plain),'--json'));assert not plain_again['created'] and plain_again['session_id']==plain_session['session_id']
            assert bar('root',plain_session['session_id']).strip()=='plain-root-scope'
            subdir=repo/'root-substitution-scope';subdir.mkdir()
            cfg.write_text(cfg.read_text()+'\n[[name_substitution]]\nfind="^.*/root-substitution-scope$"\nreplace="mapped-root-subdir"\nregex=true\n')
            substituted=json.loads(bar('connect',str(subdir),'--json'));sessions.append(substituted['session_id'])
            assert bar('root',substituted['session_id']).strip()=='mapped-root-subdir'
            rooted=json.loads(bar('connect','--root',str(subdir),'--json'));assert not rooted['created'] and rooted['session_id']==clone['session_id'];assert pathlib.Path(rooted['path']).resolve()==repo.resolve()
            first=json.loads(bar('worktree','connect','409','--repo','joshmedeski/sesh','--json'));sessions.append(first['session_id']);assert first['created']
            path=pathlib.Path(first['path']);assert path.name=='409';assert (path/'.git').is_file()
            # Read terminal output to let setup complete; markers may be written asynchronously.
            import time
            deadline=time.monotonic()+5
            while time.monotonic()<deadline and not (path/'created-marker').exists():time.sleep(0.05)
            assert (path/'created-marker').read_text()=='created';assert not (path/'startup-marker').exists()
            again=json.loads(bar('worktree','connect','409','--repo','joshmedeski/sesh','--json'));assert again['session_id']==first['session_id'] and not again['created']
            rows=json.loads(bar('worktree','list','--repo','joshmedeski/sesh','--json','--refresh'));row=next(r for r in rows if r['number']==409)
            assert 'worktree' in row['title'].lower();assert row['state']=='CLOSED';assert row['icon']=='🌿'
            url=run([*base,'worktree','browse','--print'],cwd=path,env=env).strip();assert url=='https://github.com/joshmedeski/sesh/issues/409',url
            cached=json.loads(bar('worktree','list','--repo','joshmedeski/sesh','--json'));assert cached==rows
            cursor=worktree_cursor_probe(binary,cfg,root,env)
            print(json.dumps({'checks':['local-repository-connect','local-git-clone-connect','git-root-name','plain-directory-root-name','subdirectory-root-name-substitution','connect-root-plain-create-and-reuse','connect-root-git-child-reuses-root','real-github-issue-metadata','local-git-worktree-create','create-command-exclusive','existing-session-reuse','title-state-refresh-and-cache','worktree-icon','browse-print-url','native-worktree-virtual-cursor-and-unmatched-filter'],'row':row,'url':url,'worktree_cursor':cursor},ensure_ascii=False,indent=2))
        finally:
            for sid in sessions:
                try:run(['rex','kill',sid])
                except Exception as error:print(f'cleanup warning: {error}')
if __name__=='__main__':main()
