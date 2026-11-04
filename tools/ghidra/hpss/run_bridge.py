import subprocess,json,sys
p=subprocess.Popen(['/Users/jakubkolcar/.local/bin/bridge-mcp-ghidra','--transport','stdio','--no-lazy'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
def call(i,m,args):
 p.stdin.write(json.dumps(dict(jsonrpc='2.0',id=i,method=m,params=args))+'\n');p.stdin.flush()
 while True:
  x=json.loads(p.stdout.readline())
  if x.get('id')==i:return x
call(1,'initialize',dict(protocolVersion='2025-06-18',capabilities={},clientInfo=dict(name='hpss',version='1')))
p.stdin.write(json.dumps(dict(jsonrpc='2.0',method='notifications/initialized',params={}))+'\n');p.stdin.flush()
call(2,'tools/call',dict(name='connect_instance',arguments=dict(project='mcpsamplebin')))
r=call(3,'tools/call',dict(name='run_ghidra_script',arguments=dict(script_name=sys.argv[1],args=' '.join(sys.argv[2:]),program='knob.bin',timeout_seconds=300,capture_output=True)))
try:
 text = r.get('result', {}).get('content', [{}])[0].get('text', str(r))
 try: decoded = json.loads(text)
 except (ValueError, TypeError): decoded = None
 print(decoded.get('console_output', text) if isinstance(decoded, dict) else text)
 if r.get('error') or r.get('result', {}).get('isError') or (isinstance(decoded, dict) and decoded.get('success') is False):
  sys.exit(1)
finally:p.terminate()
