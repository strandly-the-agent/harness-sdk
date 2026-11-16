import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
process.chdir(mkdtempSync(join(tmpdir(), 'r12i-')))
const model = new MockMessageModel()
model.addTurn([{ type: 'toolUseBlock', name: 'subagent', toolUseId: 'tu-1', input: { task: 'run echo' } }])
model.addTurn({ type: 'textBlock', text: 'parent waits' })
model.addTurn([{ type: 'toolUseBlock', name: 'shell', toolUseId: 'tu-c1', input: { command: 'echo hi-from-child' } }])
model.addTurn({ type: 'textBlock', text: 'child final' })
model.addTurn({ type: 'textBlock', text: 'parent final' })
model.addTurn({ type: 'textBlock', text: 'spare 1' })
model.addTurn({ type: 'textBlock', text: 'spare 2' })
const captured: string[] = []
const original = model.stream.bind(model)
;(model as any).stream = function (messages: any, options: any) {
  const wire = JSON.parse(JSON.stringify(messages)) as any[]
  captured.push(wire.map((m) => `${m.role}[${m.content.map((b: any) => Object.keys(b)[0]).join(',')}]`).join(' | '))
  return original(messages, options)
}
const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, interventions: 'ask' })
let args: any = 'go'
for (let round = 0; round < 6; round++) {
  const result = await agent.invoke(args)
  console.log(`ROUND ${round} stop=${result.stopReason} interrupts=${JSON.stringify((result.interrupts ?? []).map((i: any) => i.id))} calls=${captured.length}`)
  if (!result.interrupts || result.interrupts.length === 0) break
  args = result.interrupts.map((i: any) => ({ interruptResponse: { interruptId: i.id, response: 'y' } }))
}
captured.forEach((c, k) => console.log(`REQ${k} roles=${c}`))
process.exit(0)
