// A stdio MCP server for the Rust stdio tests, modelled on harness-ts/test/tools/echo-mcp-server.mjs.
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { z } from 'zod'

const text = (value) => ({ content: [{ type: 'text', text: value }] })

const server = new McpServer({ name: 'Echo Test Server', version: '0.0.0' })

server.registerTool('echo', { description: 'Echoes the given text back', inputSchema: { text: z.string() } }, (a) =>
  text(`echo:${a.text}`)
)
server.registerTool('add', { description: 'Adds two integers', inputSchema: { a: z.number(), b: z.number() } }, (a) =>
  text(String(a.a + a.b))
)
server.registerTool('env', { description: 'Reads an environment variable', inputSchema: { name: z.string() } }, (a) =>
  text(process.env[a.name] ?? '<unset>')
)
server.registerTool('cwd', { description: 'Reports the working directory', inputSchema: {} }, () => text(process.cwd()))
server.registerTool('pid', { description: 'Reports the process id', inputSchema: {} }, () => text(String(process.pid)))

await server.connect(new StdioServerTransport())
