import { readFileSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { z } from 'zod'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
import { tool } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/index.node.js'
for (const k of ['AWS_REGION', 'AWS_DEFAULT_REGION', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'ANTHROPIC_BASE_URL', 'OPENAI_BASE_URL', 'AWS_BEARER_TOKEN_BEDROCK']) delete process.env[k]
process.env.AWS_REGION = 'us-west-2'; process.env.ANTHROPIC_API_KEY = 'k'; process.env.OPENAI_API_KEY = 'k'
const cases = JSON.parse(readFileSync('./mcases.json', 'utf8')) as any[]
process.chdir(mkdtempSync(join(tmpdir(), 'r12m-')))
for (let i = 0; i < cases.length; i++) {
  const c = { ...cases[i] }
  const options: any = { session: false, memory: false, skills: false, printer: false, ...c }
  if (c.model === undefined) options.model = new MockMessageModel()
  if (Array.isArray(c.tools)) options.tools = (c.tools as string[]).map((name) => tool({ name, description: 'consumer', inputSchema: z.object({}), callback: () => 'ok' }))
  try {
    const agent = await createHarness(options)
    console.log(`${i}\tok tools=${JSON.stringify(agent.tools.map((t: any) => t.name))}`)
  } catch (e) {
    console.log(`${i}\terr ${(e as Error).message}`)
  }
}
process.exit(0)
