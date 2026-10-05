"""Verify signal cleanup releases the owned parent and grandchild listener."""

import subprocess,sys,socket,time,tempfile,json,pathlib
helper=pathlib.Path('extensions/cloudflare/examples/feature-worker/owned_process.py')
with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
grandchild="import socket,time; s=socket.socket(); s.bind(('127.0.0.1',"+str(port)+")); s.listen(); time.sleep(90)"
parent="import subprocess,sys,time; subprocess.Popen([sys.executable,'-c',"+repr(grandchild)+"]); time.sleep(90)"
with tempfile.TemporaryDirectory() as d:
 p=subprocess.Popen([sys.executable,str(helper),d+'/log','--',sys.executable,'-c',parent],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 try:
  deadline=time.monotonic()+10
  while True:
   try:
    with socket.create_connection(('127.0.0.1',port),timeout=.2):break
   except OSError:
    if time.monotonic()>deadline:raise
    time.sleep(.05)
  p.terminate();out,err=p.communicate(timeout=15)
  assert p.returncode==143, f'expected 143, got {p.returncode}: {err}'
  assert not err,err
  with socket.socket() as s:s.bind(('127.0.0.1',port))
  print(json.dumps({'exitCode':p.returncode,'stderrEmpty':True,'grandchildPortClosed':True}))
 finally:
  if p.poll() is None:p.terminate();p.wait(timeout=15)
