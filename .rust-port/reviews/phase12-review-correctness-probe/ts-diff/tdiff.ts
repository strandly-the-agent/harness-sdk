import { readFileSync } from 'node:fs'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
const cases = JSON.parse(readFileSync('./tcases.json', 'utf8')) as [string, unknown][]
process.chdir('/tmp/review12_correctness/fx')
const model = new MockMessageModel()
cases.forEach(([name, input], k) => model.addTurn([{ type: 'toolUseBlock', name, toolUseId: `t${k}`, input: input as never }]))
model.addTurn({ type: 'textBlock', text: 'done' })
const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, backgroundTasks: false, contextManager: null })
await agent.invoke('go')
const wire = JSON.parse(JSON.stringify(agent.messages)) as any[]
for (const m of wire) for (const b of m.content) if (b.toolResult) {
  const k = Number(b.toolResult.toolUseId.slice(1))
  const content = b.toolResult.content.map((c: any) => (c.text !== undefined ? c.text : JSON.stringify(Object.keys(c)) + JSON.stringify(c).length))
  console.log(`${k}\t${cases[k]![0]}\t${b.toolResult.status}\t${JSON.stringify(content)}`)
}
