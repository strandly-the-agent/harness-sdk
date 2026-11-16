import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
process.chdir(mkdtempSync(join(tmpdir(), 'r12c-')))
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const model = new MockMessageModel()
model.addTurn([{ type: 'toolUseBlock', name: 'subagent', toolUseId: 'tu-1', input: { task: 'count files' } }])
model.addTurn({ type: 'textBlock', text: 'parent waits' })
model.addTurn({ type: 'textBlock', text: 'child done' })
model.addTurn({ type: 'textBlock', text: 'turn four' })
model.addTurn({ type: 'textBlock', text: 'turn five' })
let calls = 0
const original = model.stream.bind(model)
;(model as any).stream = async function* (messages: any, options: any) {
  const n = ++calls
  if (n === 3) await sleep(3000)
  yield* original(messages, options)
}
const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false })
const started = Date.now()
setTimeout(() => agent.cancel(), 500)
const result = await agent.invoke('go')
console.log(`CANCEL after ${Date.now() - started}ms: ${result.stopReason} ${JSON.stringify(result.toString())} calls=${calls}`)
await sleep(4000)
console.log(`CALLS 4s later ${calls}`)
const again = await agent.invoke('next')
console.log(`NEXT ${again.stopReason} ${JSON.stringify(again.toString())} calls=${calls}`)
for (const m of JSON.parse(JSON.stringify(agent.messages))) console.log('MSG ' + JSON.stringify(m).slice(0, 330))
process.exit(0)
