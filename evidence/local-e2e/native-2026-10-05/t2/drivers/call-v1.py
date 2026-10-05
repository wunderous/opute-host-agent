import hashlib, http.client, json, pathlib, sys, time
impl, method, params_text, label = sys.argv[1:]
config=dict(line.split('=',1) for line in pathlib.Path('/opt/parity/'+impl+'.env').read_text().splitlines())
params=json.loads(params_text)
params['_meta']={'io.modelcontextprotocol/protocolVersion':'2026-07-28','io.modelcontextprotocol/clientInfo':{'name':'native-t2','version':'1'},'io.modelcontextprotocol/clientCapabilities':{'extensions':{'io.modelcontextprotocol/tasks':{}}}}
headers={'Content-Type':'application/json','Accept':'application/json, text/event-stream','Authorization':'Bearer '+config['MCP_AUTH_TOKEN'],'MCP-Protocol-Version':'2026-07-28','Mcp-Method':method}
if params.get('name'): headers['Mcp-Name']=params['name']
request={'jsonrpc':'2.0','id':1,'method':method,'params':params}
connection=http.client.HTTPConnection('127.0.0.1',int(config['HOST_MCP_PORT']),timeout=1000)
started=time.time()
connection.request('POST','/mcp',json.dumps(request),headers)
response=connection.getresponse(); raw=response.read(); connection.close()
try: body=json.loads(raw)
except ValueError: body={'raw':raw.decode(errors='replace')}
record={'impl':impl,'request':request,'status':response.status,'body':body,'elapsedSeconds':time.time()-started,'driverSha256':hashlib.file_digest(pathlib.Path(__file__).open('rb'),'sha256').hexdigest()}
path=pathlib.Path('/opt/parity/observations'); path.mkdir(exist_ok=True)
(path/(label+'.json')).write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
