import concurrent.futures, hashlib, http.client, http.server, json, pathlib, sys, threading, time
impl=sys.argv[1]
config=dict(line.split('=',1) for line in pathlib.Path('/opt/parity/'+impl+'.env').read_text().splitlines())
records={};checks=[]
def call(label,tool,args=None):
 params={'name':tool,'arguments':args or {},'_meta':{'io.modelcontextprotocol/protocolVersion':'2026-07-28','io.modelcontextprotocol/clientInfo':{'name':'native-t2','version':'1'},'io.modelcontextprotocol/clientCapabilities':{'extensions':{'io.modelcontextprotocol/tasks':{}}}}}
 c=http.client.HTTPConnection('127.0.0.1',int(config['HOST_MCP_PORT']),timeout=60)
 c.request('POST','/mcp',json.dumps({'jsonrpc':'2.0','id':1,'method':'tools/call','params':params}),{'Content-Type':'application/json','Accept':'application/json, text/event-stream','Authorization':'Bearer '+config['MCP_AUTH_TOKEN'],'MCP-Protocol-Version':'2026-07-28','Mcp-Method':'tools/call','Mcp-Name':tool})
 r=c.getresponse();raw=r.read();c.close();body=json.loads(raw)
 records[label]={'status':r.status,'request':params,'body':body}
 assert r.status==200 and 'error' not in body,records[label]
 return body['result']
def good(label,tool,args=None):
 r=call(label,tool,args);assert not r.get('isError'),r
 return r['structuredContent']
def check(name,condition):
 assert condition,name
 checks.append(name)
file=pathlib.Path('/root/native-t2-owned.txt');contents=b'owned native T2 file\n';file.write_bytes(contents)
try:
 info=good('host-info','get_host_info');check('exact opaque identity',info['agent']['agentId']==config['OPUTE_REMOTE_AGENT_ID'])
 platform=good('platform','detect_host_platform');check('native Linux',platform['os']=='linux' and platform['kind']!='wsl')
 cap=good('capacity','get_host_capacity');check('real enforcement',cap['enforcement']=='enforced');check('physical capacity cap',0<cap['effectiveLimits']['memoryBytes']<=6<<30)
 for iteration in range(5):
  inventory=good('inventory-'+str(iteration),'list_vms',{'fast':True});items=inventory['vms'];owned=next(x for x in items if x['name']=='parity-native-read')
  check('canonical inventory '+str(iteration),owned['uri']=='container:local:parity-native-read' and owned['kind']=='container' and owned['status']=='running' and owned['hostId']==config['OPUTE_REMOTE_AGENT_ID'])
  target=good('bound-read-'+str(iteration),'get_vm_info',{'uri':'container:local:parity-native-read','fast':True});check('canonical bound read '+str(iteration),target['uri']=='container:local:parity-native-read' and target['cpus']==1)
 for label,uri in [('wrong-kind','vm:local:parity-native-read'),('foreign-tenant','container:other:parity-native-read'),('missing-target','container:local:parity-native-missing')]:
  rejected=call(label,'get_vm_info',{'uri':uri,'fast':True});check(label+' fail closed',rejected.get('isError') is True and rejected['structuredContent']['owner']=='admission')
 read=good('file-before','inspect_host_file',{'path':str(file)});check('real file SHA',read['sha256']=='sha256:'+hashlib.sha256(contents).hexdigest())
 gate=threading.Event();arrived=threading.Condition();count=[0]
 class Handler(http.server.BaseHTTPRequestHandler):
  def do_GET(self):
   with arrived: count[0]+=1;arrived.notify_all()
   assert gate.wait(12),'release gate timeout'
   self.send_response(204);self.end_headers()
  def log_message(self,*args): pass
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
 threading.Thread(target=server.serve_forever,daemon=True).start()
 try:
  with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
   futures=[pool.submit(good,'held-http-'+str(i),'probe_http_endpoint',{'endpoint':f'http://127.0.0.1:{server.server_port}/hold'}) for i in range(2)]
   try:
    with arrived: check('two actual HTTP requests held',arrived.wait_for(lambda:count[0]==2,timeout=8))
    capacity=good('capacity-held','get_host_capacity');check('two real leases held',capacity['reservations']['count']==2)
    refused=call('saturation','inspect_host_file',{'path':str(file)});check('actual saturation refusal',refused.get('isError') is True and refused['structuredContent']['code']=='host_capacity_saturated')
    good('control-held','get_capability_catalog');checks.append('control admitted while saturated')
   finally: gate.set()
   for future in futures: check('released HTTP ready',future.result(timeout=20)['ready'] is True)
 finally: gate.set();server.shutdown();server.server_close()
 read=good('file-after','inspect_host_file',{'path':str(file)});check('admitted after release',read['sha256']=='sha256:'+hashlib.sha256(contents).hexdigest())
 check('owned file unchanged',file.read_bytes()==contents)
 report={'impl':impl,'checks':checks,'records':records,'driverSha256':hashlib.file_digest(pathlib.Path(__file__).open('rb'),'sha256').hexdigest(),'binarySha256':hashlib.file_digest(pathlib.Path('/home/ubuntu/'+('go-reference' if impl=='goref' else 'opute-host-agent')).open('rb'),'sha256').hexdigest(),'scope':'real Incus and systemd enforcement inside independently leased disposable QEMU clone; no executable shims','status':'pass'}
 pathlib.Path('/opt/parity/native-checks-'+impl+'.json').write_text(json.dumps(report,indent=2)+'\n')
 print(impl,len(checks),'native checks passed')
finally: file.unlink(missing_ok=True)
