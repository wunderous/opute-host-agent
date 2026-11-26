#!/usr/bin/env node
// Scripted MCP client used in the README demo: no AI, every call is typed
// by hand. It speaks stateless MCP 2026-07-28 to the agent.
//   mcp tools [filter]          list tools with their read-only/destructive hints
//   mcp call <tool> [json]      call a tool and print its structured result
//   mcp schema <tool>           print a tool's full definition
// Destructive tools need a client with the tasks extension; this one has none.
// The bearer token comes from MCP_AUTH_TOKEN and is never printed.
const url = process.env.MCP_URL || 'http://127.0.0.1:3014/mcp'
const token = process.env.MCP_AUTH_TOKEN
if (!token) { console.error('MCP_AUTH_TOKEN is not set'); process.exit(2) }
let nextId = 1

async function rpc(method, params = {}) {
  const headers = {
    Accept: 'application/json, text/event-stream',
    'Content-Type': 'application/json',
    'MCP-Protocol-Version': '2026-07-28',
    'Mcp-Method': method,
    Origin: new URL(url).origin,
    Authorization: `Bearer ${token}`,
  }
  if (method === 'tools/call') headers['Mcp-Name'] = params.name
  const body = {
    jsonrpc: '2.0', id: nextId++, method,
    params: {
      ...params,
      _meta: {
        'io.modelcontextprotocol/protocolVersion': '2026-07-28',
        'io.modelcontextprotocol/clientCapabilities': {},
        'io.modelcontextprotocol/clientInfo': { name: 'm29-demo-client', version: '1' },
      },
    },
  }
  const res = await fetch(url, { method: 'POST', headers, body: JSON.stringify(body) })
  const text = await res.text()
  if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText}`)
  const env = JSON.parse(text)
  if (env.error) throw new Error(`${env.error.message}` + (process.env.MCP_DEBUG ? ' ' + JSON.stringify(env.error.data) : ''))
  return env.result
}

async function allTools() {
  const tools = []
  let cursor
  do {
    const r = await rpc('tools/list', cursor ? { cursor } : {})
    tools.push(...r.tools)
    cursor = r.nextCursor
  } while (cursor)
  return tools
}

const [cmd, a, b] = process.argv.slice(2)
try {
if (cmd === 'tools') {
  const tools = await allTools()
  const shown = a ? tools.filter(t => t.name.includes(a)) : tools
  for (const t of shown) {
    const ro = t.annotations?.readOnlyHint ? 'read-only' : (t.annotations?.destructiveHint ? 'destructive' : 'change')
    console.log(`${t.name.padEnd(34)} ${ro}`)
  }
  if (!a) console.log(`(${tools.length} tools)`)
} else if (cmd === 'schema') {
  const t = (await allTools()).find(t => t.name === a)
  console.log(JSON.stringify(t, null, 2))
} else if (cmd === 'call') {
  const r = await rpc('tools/call', { name: a, arguments: b ? JSON.parse(b) : {} })
  if (r.isError) {
    console.log('refused:', r.content?.map(c => c.text).join(' ') || JSON.stringify(r.structuredContent))
    process.exitCode = 1
  } else {
    console.log(JSON.stringify(r.structuredContent ?? r.content, null, 2))
  }
} else {
  console.error('usage: mcp tools [filter] | mcp call <tool> [json] | mcp schema <tool>')
  process.exit(2)
}
} catch (e) {
  console.log(`refused: ${e.message}`)
  process.exitCode = 1
}
