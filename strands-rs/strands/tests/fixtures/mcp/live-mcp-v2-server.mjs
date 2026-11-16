// MCP test server (protocol revision 2026-07-28) over stdio for tests/live_mcp.rs: the tools of
// strands-ts/test/integ/__fixtures__/test-mcp-v2-server.ts, in plain JavaScript.
import { acceptedContent, inputRequired, inputResponse, McpServer } from '@modelcontextprotocol/server'
import { serveStdio } from '@modelcontextprotocol/server/stdio'
import * as z from 'zod/v4'

function createTestServer() {
  const confirmationSchema = z.object({ confirmed: z.boolean().describe('Whether the user confirms') })
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
    ({ action }, context) => {
      const response = inputResponse(context.mcpReq.inputResponses, 'confirmation')
      if (response.kind === 'missing') {
        return inputRequired({
          inputRequests: {
            confirmation: inputRequired.elicit({
              message: `Do you want to proceed with: ${action}?`,
              requestedSchema: confirmationSchema,
            }),
          },
        })
      }
      if (response.kind !== 'elicit') {
        throw new Error('Confirmation input returned an unexpected response type')
      }
      if (response.action !== 'accept') {
        const outcome = response.action === 'cancel' ? 'cancelled' : 'declined'
        return { content: [{ type: 'text', text: `Action "${action}" was ${outcome} by user` }] }
      }
      const content = acceptedContent(context.mcpReq.inputResponses, 'confirmation', confirmationSchema)
      return content?.confirmed
        ? { content: [{ type: 'text', text: `Action "${action}" confirmed by user` }] }
        : { content: [{ type: 'text', text: `Action "${action}" was declined by user` }] }
    }
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

  return server
}

void serveStdio(createTestServer)
