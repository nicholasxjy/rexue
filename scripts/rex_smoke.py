#!/usr/bin/env python3
"""Opt-in Rex acceptance: observe app-selected tabs, outputs and isolated resources."""
import argparse,json,os,pathlib,plistlib,subprocess,tempfile,time,uuid
from rex_client_probe import focused_block

def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',default='target/debug/rexue');p.add_argument('--client',required=True);p.add_argument('--rex',default='rex');p.add_argument('--require-activation',action='store_true');a=p.parse_args()
    binary=str(pathlib.Path(a.binary).resolve());created=[];checks=[];frames={};display_probes=[]
    def run(args,env=None):
        r=subprocess.run(args,text=True,capture_output=True,timeout=35,env=env)
        if r.returncode or 'could not activate configured application' in r.stderr:raise RuntimeError(f'{args[0]} failed ({r.returncode}): {r.stderr}')
        return r.stdout
    def rex(*args):return run([a.rex,*args])
    def front_app():return json.loads(run(['osascript','-l','JavaScript','-e','ObjC.import("AppKit"); var a=$.NSWorkspace.sharedWorkspace.frontmostApplication; JSON.stringify({name:ObjC.unwrap(a.localizedName),pid:a.processIdentifier,path:ObjC.unwrap(a.bundleURL.path)})']))
    def activate_app(name):return run(['/usr/bin/open','-a',name])
    original_app=front_app();activation_receipt={}
    registry=plistlib.loads(subprocess.run(['ioreg','-n','Root','-d','1','-a'],capture_output=True,check=True,timeout=5).stdout)
    if isinstance(registry,list):registry=registry[0]
    desktop_locked=bool(registry.get('IOConsoleLocked')) or any(u.get('CGSSessionScreenIsLocked') for u in registry.get('IOConsoleUsers',[]) if u.get('kCGSSessionOnConsoleKey'))
    if a.require_activation and desktop_locked:raise RuntimeError('macOS screen is locked; unlock it before requiring foreground activation')
    client=json.loads(rex('client','inspect',a.client,'--json'))['client'];assert client['principal']['transport']=='unix' and client['info']['platform']=='macos','activation smoke requires a local macOS app client'
    target_pid=client['principal']['pid']
    target_app=json.loads(run(['osascript','-l','JavaScript','-e','ObjC.import("AppKit"); var a=$.NSRunningApplication.runningApplicationWithProcessIdentifier('+str(int(target_pid))+'); JSON.stringify({name:ObjC.unwrap(a.localizedName),pid:a.processIdentifier,path:ObjC.unwrap(a.bundleURL.path)})']))
    def inspect(sid):return json.loads(rex('session','inspect',sid,'--json'))
    def capture(sid,block):return rex('capture','-s',sid,'-b',block,'--trim','--unwrap')
    before={s['session_id'] for s in json.loads(rex('ls','--json'))['sessions']}
    original_block=focused_block(a.client,a.rex);original=None;original_window=None
    for session_id in before:
        for window in inspect(session_id)['windows']:
            blocks=[b['block_id'] for layer in window['layers'] for b in layer['blocks']]
            if original_block in blocks:original=session_id;original_window=window['window_id']
    assert original and original_window,'cannot identify the originally displayed client pane'
    def wait_for(predicate,seconds=6):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if predicate():return
            time.sleep(0.05)
        raise AssertionError('condition timed out; frames='+str(frames))
    def probe_display(sid):
        # Client view state is independent of server active_window_id. Observe
        # the app's focused pane via its copy-ID action and compare real IDs.
        windows=inspect(sid)['windows'];assert len(windows)>=2
        pane_ids={b['block_id'] for w in windows for layer in w['layers'] for b in layer['blocks']}
        observed=[]
        def shown():
            value=focused_block(a.client,a.rex);observed.append(value);return value in pane_ids
        wait_for(shown);on_select=observed[-1]
        display_probes.append({'session_id':sid,'expected_session_blocks':sorted(pane_ids),'observed_focused_block':on_select})
    with tempfile.TemporaryDirectory(prefix='rexue-live-') as directory:
        root=pathlib.Path(directory);project=root/'space $(literal) 项目';project.mkdir();name='rexue-test-'+uuid.uuid4().hex[:8]
        cfg=root/'config.toml'
        nested='rexue_path() { printf \'%s\' "$(basename \'{}\')"; }; set -- changed; rexue_path other'
        startup=r"printf 'rexue-live-preview-ready\n'; printf 'startup\n' >> {}/startup-count; "+nested+" > {}/nested-startup"
        window_startup=r"printf 'logs-ready\n'; "+nested+" > {}/nested-window"
        cfg.write_text('strict_mode=true\nterminal='+json.dumps(target_app['path'])+'\n[tui]\npreview_min_width=60\npreview=true\n[default_session]\nstartup_command='+json.dumps(startup)+'\nwindows=["Logs"]\n[frecency]\nlist_command="true"\nquery_command="false"\nadd_command="true"\nremove_command="true"\n[[session]]\nname='+json.dumps(name)+'\nalias="wp"\npath='+json.dumps(str(project))+'\nstartup_command=""\nwindows=[]\n[[window]]\nname="Logs"\nstartup_script='+json.dumps(window_startup)+'\n')
        env=dict(os.environ,REXUE_STATE_DIR=str(root/'state'),REXUE_OPEN='/usr/bin/open');base=[binary,'--client',a.client,'-C',str(cfg)]
        def bar(*args):return run([*base,*args],env)
        try:
            if not desktop_locked:
                activate_app('Finder');wait_for(lambda:front_app()['name']=='Finder')
            c=json.loads(bar('connect','WP','--json'));sid=c['session_id'];created.append(sid);assert c['created']
            activation_receipt={'requested':target_app['path'],'request_accepted':True,'screen_locked':desktop_locked}
            if desktop_locked:activation_receipt.update(foreground_verification='unverified_screen_locked',observed_frontmost=front_app())
            else:
                wait_for(lambda:front_app()['pid']==target_app['pid']);activation_receipt.update(foreground_verification='verified',before='Finder',observed_frontmost=front_app());checks.append('configured-terminal-activates-macos-app')
            detail=inspect(sid);assert [w['label'] for w in detail['windows']]==['Terminal','Logs'];checks.append('empty-session-values-inherit-default-startup-and-windows')
            log_block=detail['windows'][1]['focused_block_id'];wait_for(lambda:'logs-ready' in capture(sid,log_block))
            wait_for(lambda:(project/'nested-startup').exists() and (project/'nested-startup').read_text()==project.name)
            wait_for(lambda:(project/'nested-window').exists() and (project/'nested-window').read_text()==project.name)
            nested_cfg=root/'nested-preview.toml'
            nested_cfg.write_text('[[session]]\nname="audit-nested"\npath='+json.dumps(str(project))+'\npreview_command='+json.dumps(nested)+'\n')
            nested_preview=run([binary,'--client',a.client,'-C',str(nested_cfg),'preview','audit-nested'],env)
            assert nested_preview==project.name
            nested_receipt={'expected':project.name,'session_startup':(project/'nested-startup').read_text(),'window_startup':(project/'nested-window').read_text(),'preview':nested_preview}
            checks.append('nested-placeholder-preview-session-and-window-startup')
            probe_display(sid)
            assert bar('window','connect','--target',sid,'Logs').strip()==detail['windows'][1]['window_id']
            wait_for(lambda:focused_block(a.client,a.rex)==log_block)
            display_probes.append({'session_id':sid,'window_name':'Logs','expected_block':log_block,'observed_focused_block':focused_block(a.client,a.rex)})
            bar('window','connect','--target',sid,'Terminal');wait_for(lambda:focused_block(a.client,a.rex)==detail['windows'][0]['focused_block_id'])
            checks.append('create-and-observe-client-selected-session-and-logs-output')
            wait_for(lambda:(project/'startup-count').exists());count=(project/'startup-count').read_text();assert count.count('startup')==1
            again=json.loads(bar('connect','wp','--command','touch SHOULD_NOT_RUN','--json'));assert again['session_id']==sid and not again['created']
            assert not (project/'SHOULD_NOT_RUN').exists();assert (project/'startup-count').read_text()==count;probe_display(sid);checks.append('reuse-without-rerunning-startup')
            wid=bar('window','connect','--target',sid,'Scratch','--background').strip();assert wid.startswith('window:')
            assert bar('window','connect','--target',sid,'Scratch','--background').strip()==wid;checks.append('window-create-and-reuse')
            bar('window','connect','--target',sid,'Terminal')
            terminal_block=detail['windows'][0]['focused_block_id']
            wait_for(lambda:focused_block(a.client,a.rex)==terminal_block)
            marker='REXUE_ASSERT_OUTPUT_'+uuid.uuid4().hex
            rex('send','-s',sid,'-b',terminal_block,"printf '%s\\n' '"+marker+"'");rex('send-key','-s',sid,'-b',terminal_block,'enter')
            wait_for(lambda:bar('preview',sid).count(marker)>=2);checks.append('live-preview-contains-executed-command-output')
            pane_env=dict(env,REX_SESSION=sid,REX_BLOCK=terminal_block)
            def pane_bar(*args):return run([*base,*args],pane_env)
            panes=json.loads(pane_bar('list','--panes','--json'));logs=next(p for p in panes if p['session_id']==log_block)
            pane_marker='REXUE_PANE_OUTPUT_'+uuid.uuid4().hex
            rex('send','-s',sid,'-b',log_block,"printf '%s\\n' '"+pane_marker+"'");rex('send-key','-s',sid,'-b',log_block,'enter')
            wait_for(lambda:pane_bar('preview',logs['name']).count(pane_marker)>=2)
            selected=json.loads(pane_bar('connect',logs['name'],'--json'));assert selected['session_id']==sid and not selected['created']
            wait_for(lambda:focused_block(a.client,a.rex)==log_block)
            union=json.loads(pane_bar('list','--panes','--config','--json'));assert {'rex-block','config'}<={e['source'] for e in union}
            block_cfg=root/'blocked.toml';block_cfg.write_text('blacklist=[".*"]\n')
            assert json.loads(run([binary,'-C',str(block_cfg),'list','--panes','--json'],pane_env))==[]
            assert len(json.loads(run([binary,'-C',str(block_cfg),'list','--panes','--blacklisted','--json'],pane_env)))==len(panes)
            bar('window','connect','--target',sid,'Terminal');checks.append('plain-pane-name-preview-connect-union-and-blacklist')
            picker=json.loads(rex('new',name+'-picker','-c',str(project),'--keep-open','--json','--',*base,'picker','-c','--preview','--no-cache'))
            pid=picker['session_id'];created.append(pid);block=picker['initial_windows'][0]['block_ids'][0]
            wait_for(lambda:'│' in capture(pid,block) and name in capture(pid,block))
            size=json.loads(rex('block','call','-s',pid,'-b',block,'size'));frames[f"{size['columns']}x{size['rows']}"]=capture(pid,block)
            rex('send-key','-s',pid,'-b',block,'esc');checks.append('native-picker-preview-render-and-cancel')
            second=json.loads(bar('mkdir',str(root/'second'),'--json'));created.append(second['session_id'])
            bar('window','connect','--target',second['session_id'],'Other','--background');probe_display(second['session_id'])
            assert bar('last').strip()==sid;probe_display(sid);checks.append('last-observably-switches-client-session')
            label=inspect(second['session_id'])['label']
            deleter=json.loads(rex('new',name+'-delete','-c',str(project),'--keep-open','--json','--',*base,'picker','-t','--no-cache','--query',label))
            did=deleter['session_id'];created.append(did);db=deleter['initial_windows'][0]['block_ids'][0]
            def frame():return capture(did,db)
            wait_for(lambda:frame().count(label)>=2);rex('send-key','-s',did,'-b',db,'ctrl+x');wait_for(lambda:'Do you want to kill this Rex session?' in frame())
            rex('send','-s',did,'-b',db,'n');assert any(s['session_id']==second['session_id'] for s in json.loads(rex('ls','--json'))['sessions'])
            rex('send-key','-s',did,'-b',db,'ctrl+x');wait_for(lambda:'Do you want to kill this Rex session?' in frame());rex('send','-s',did,'-b',db,'y')
            wait_for(lambda:all(s['session_id']!=second['session_id'] for s in json.loads(rex('ls','--json'))['sessions']));created.remove(second['session_id']);rex('send-key','-s',did,'-b',db,'esc')
            checks.append('cancel-and-confirm-live-session-close')
            print(json.dumps({'checks':checks,'client_display_probes':display_probes,'application_activation':activation_receipt,'nested_shell_placeholder':nested_receipt,'frames':frames,'session_ids':created},ensure_ascii=False,indent=2))
        finally:
            if original and original in before:
                try:rex('-C',a.client,'do','session.select','--args',json.dumps({'session_id':original,'window_id':original_window}),'--json')
                except Exception as e:print(f'restore warning: {e}')
            for sid in reversed(created):
                if sid not in before:
                    try:rex('kill',sid)
                    except Exception as e:print(f'cleanup warning: {e}')
            if not desktop_locked and original_app.get('path'):
                try:activate_app(original_app['path'])
                except Exception as e:print(f'app restore warning: {e}')
if __name__=='__main__':main()
