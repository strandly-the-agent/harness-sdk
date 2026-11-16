// MCP test server over stdio for tests/live_mcp.rs: the tools, prompt and resources of
// strands-ts/test/integ/__fixtures__/test-mcp-server.ts, in plain JavaScript.
import { McpServer, ResourceTemplate } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import * as z from 'zod/v4'

const server = new McpServer({ name: 'test-mcp-server', version: '1.0.0' }, { capabilities: { tools: {} } })

server.registerTool(
  'echo',
  {
    title: 'Echo Tool',
    description: 'Echoes back the input message',
    inputSchema: { message: z.string() },
    outputSchema: { echo: z.string() },
  },
  async ({ message }) => ({ content: [{ type: 'text', text: message }], structuredContent: { echo: message } })
)

server.registerTool(
  'calculator',
  {
    title: 'Calculator Tool',
    description: 'Performs basic arithmetic operations',
    inputSchema: { operation: z.enum(['add', 'subtract', 'multiply', 'divide']), a: z.number(), b: z.number() },
    outputSchema: { result: z.number() },
  },
  async ({ operation, a, b }) => {
    if (operation === 'divide' && b === 0) throw new Error('Division by zero')
    const result = { add: a + b, subtract: a - b, multiply: a * b, divide: a / b }[operation]
    return { content: [{ type: 'text', text: `Result: ${result}` }], structuredContent: { result } }
  }
)

server.registerTool(
  'confirm_action',
  {
    title: 'Confirm Action Tool',
    description: 'Asks the user to confirm before proceeding. Use this tool when you need user confirmation.',
    inputSchema: { action: z.string() },
  },
  async ({ action }) => {
    const result = await server.server.elicitInput({
      message: `Do you want to proceed with: ${action}?`,
      requestedSchema: {
        type: 'object',
        properties: { confirmed: { type: 'boolean', description: 'Whether the user confirms' } },
      },
    })
    const text =
      result.action === 'accept'
        ? `Action "${action}" confirmed by user`
        : `Action "${action}" was ${result.action}d by user`
    return { content: [{ type: 'text', text }] }
  }
)

server.registerPrompt(
  'summarize',
  {
    title: 'Summarize Prompt',
    description: 'Builds a summarization request for a topic',
    argsSchema: { topic: z.string() },
  },
  ({ topic }) => ({ messages: [{ role: 'user', content: { type: 'text', text: `Summarize the topic: ${topic}` } }] })
)

server.registerResource(
  'greeting',
  'test://greeting.txt',
  { title: 'Greeting Resource', description: 'A static text resource', mimeType: 'text/plain' },
  async (uri) => ({ contents: [{ uri: uri.href, text: 'Hello from the test resource', mimeType: 'text/plain' }] })
)

server.registerResource(
  'user-file',
  new ResourceTemplate('test://users/{userId}', { list: undefined }),
  { title: 'User File Template', description: 'A templated resource addressed by user id' },
  async (uri, { userId }) => ({
    contents: [{ uri: uri.href, text: `Data for user ${String(userId)}`, mimeType: 'text/plain' }],
  })
)

server.registerTool(
  'error_tool',
  {
    title: 'Error Tool',
    description: 'Intentionally throws an error for testing error handling',
    inputSchema: { error_message: z.string().optional() },
    outputSchema: { error: z.string() },
  },
  async ({ error_message }) => {
    throw new Error(error_message || 'Intentional error')
  }
)

await server.connect(new StdioServerTransport())
