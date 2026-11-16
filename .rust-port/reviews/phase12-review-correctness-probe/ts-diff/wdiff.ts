import { readFileSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
const transport = process.argv[2]
const cases = JSON.parse(readFileSync('/tmp/review12_correctness/web/wcases.json', 'utf8')) as unknown[]
process.chdir(mkdtempSync(join(tmpdir(), 'r12web-')))
for (let k = 0; k < cases.length; k++) {
  const model = new MockMessageModel()
  model.addTurn([{ type: 'toolUseBlock', name: 'web_fetch', toolUseId: `t${k}`, input: cases[k] as never }])
  model.addTurn({ type: 'textBlock', text: 'SUMMARY-OR-DONE' })
  model.addTurn({ type: 'textBlock', text: 'done2' })
  const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, backgroundTasks: false, contextManager: null, builtinTools: { '*': false, web_fetch: { transport } } as never, builtinPlugins: [] })
  await agent.invoke('go')
  const wire = JSON.parse(JSON.stringify(agent.messages)) as any[]
  for (const m of wire) for (const b of m.content) if (b.toolResult) {
    const text = b.toolResult.content.map((c: any) => c.text ?? JSON.stringify(c)).join('|')
    console.log(`${k}\t${b.toolResult.status}\t${text.length}\t${JSON.stringify(text.slice(0, 300))}\t${JSON.stringify(text.slice(-80))}`)
  }
}
process.exit(0)
