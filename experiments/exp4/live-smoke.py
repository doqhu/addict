import subprocess,pathlib,json,time,os

def _smoke_failed(row):
 return (row.get('error') is not None or row.get('presented_record') is not True
         or any(row.get(key) != 0 for key in ('invoke_exit', 'quit_exit', 'exit_code')))

def _session_blocker():
 # A locked or blanked session presents no frames and grants no focus; such a run measures nothing.
 try:
  session=subprocess.run(['loginctl','show-user',str(os.getuid()),'-p','Display','--value'],capture_output=True,text=True,timeout=5).stdout.strip()
  hints=subprocess.run(['loginctl','show-session',session,'-p','LockedHint','-p','IdleHint'],capture_output=True,text=True,timeout=5).stdout.split()
 except (OSError,subprocess.SubprocessError) as e:
  return None if isinstance(e,FileNotFoundError) else str(e)
 return ', '.join(h for h in hints if h.endswith('=yes')) or None

def main():
 blocker=_session_blocker()
 if blocker:
  print(json.dumps({'error':'desktop session not presenting: '+blocker}))
  return 2
 failed=False
 root=pathlib.Path(__file__).resolve().parents[2]
 results=root/'_bmad-output/initiative-linux-rust-translator/experiment-exp-4/results.jsonl'
 results.parent.mkdir(parents=True,exist_ok=True)
 harness=root/'target/release/addict-exp4'
 for backend,frontend in [('gtk4',root/'target/release/addict-ui-gtk'),('gpui',root/'crates/addict-ui-gpui/target/release/addict-ui-gpui')]:
  for mode in ['resident','on-demand']:
   directory=pathlib.Path('/tmp')/f'exp4-live-{os.getpid()}-{backend}-{mode}';directory.mkdir(mode=0o700)
   socket=directory/'broker.sock';log=open(directory/'stderr.log','w')
   args=[str(harness),'broker','--backend',backend,'--mode',mode,'--origin','local','--frontend',str(frontend),'--socket',str(socket),'--results',str(results),'--local-only','--timeout-seconds','15','--fixture',str(root/'experiments/exp4/ocr-fixture.png')]
   child=None
   try:
    child=subprocess.Popen(args,cwd=root,stdout=log,stderr=log)
    deadline=time.monotonic()+7
    while not socket.exists() and child.poll() is None and time.monotonic()<deadline:time.sleep(.03)
    if not socket.exists():raise RuntimeError('broker socket missing')
    invoke=subprocess.run([str(harness),'invoke','--socket',str(socket)],capture_output=True,text=True,timeout=5)
    if invoke.returncode:raise RuntimeError(invoke.stderr)
    deadline=time.monotonic()+5
    presented=False;records=[]
    while time.monotonic()<deadline and child.poll() is None:
     lines=[json.loads(l) for l in results.read_text().splitlines()]
     records=[r for r in lines if r['backend']==backend and r['mode']==mode and str(child.pid)+'-' in (r['invocation_id']or'')]
     if any(r['event']=='presentation' for r in records):presented=True;break
     time.sleep(.03)
    quit=subprocess.run([str(harness),'quit','--socket',str(socket)],capture_output=True,text=True,timeout=5)
    child.wait(timeout=6)
    lines=[json.loads(l) for l in results.read_text().splitlines()]
    records=[r for r in lines if r['backend']==backend and r['mode']==mode and str(child.pid)+'-' in (r['invocation_id']or'')]
    row={'backend':backend,'mode':mode,'broker_pid':child.pid,'presented_record':presented,'invoke_exit':invoke.returncode,'exit_code':child.returncode,'quit_exit':quit.returncode,'stderr':str(directory/'stderr.log')}
    row['focus_observed']=[r['details'].get('focused') for r in records if r['event']=='frontend-event' and r['details'].get('event')=='focus']
    row['focus_limitation']=None if row['focus_observed'] else 'no focus observation recorded'
    row['presentation_feedback']=[{key:r['details'].get(key) for key in ('time_ns','clock_id','limitation')} for r in records if r['event']=='presentation']
    failed |= _smoke_failed(row)
    print(json.dumps(row))
   except Exception as e:
    row={'backend':backend,'mode':mode,'error':str(e),'stderr':str(directory/'stderr.log')}
    failed |= _smoke_failed(row)
    print(json.dumps(row))
   finally:
    if child is not None and child.poll() is None:child.kill();child.wait(timeout=5)
    log.close()
 return int(failed)

if __name__ == '__main__':
 raise SystemExit(main())
