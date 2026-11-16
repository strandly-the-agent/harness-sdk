import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { EnvironmentContext, Todos } from './harness-ts/src/plugins/index.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
process.chdir(mkdtempSync(join(tmpdir(), 'r12e-')))
const model = new MockMessageModel()
model.addTurn({ type: 'textBlock', text: 'done' })
let captured: any
const original = model.stream.bind(model)
;(model as any).stream = function (messages: any, options: any) { captured ??= JSON.stringify(messages); return original(messages, options) }
const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, plugins: [new EnvironmentContext({ name: 'my:env' }), new Todos({ name: 'my:todos' })] })
await agent.invoke('go')
console.log('TS ENVIRONMENT BLOCKS: ' + (captured.match(/<environment>/g) ?? []).length + ' tools=' + JSON.stringify(agent.tools.map((t: any) => t.name)))
process.exit(0)
