import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
process.chdir('/tmp/review12_correctness/ts/cwd6')
const model = new MockMessageModel()
model.addTurn({ type: 'textBlock', text: 'done' })
let captured: any
const original = model.stream.bind(model)
;(model as any).stream = function (messages: any, options: any) { captured ??= JSON.parse(JSON.stringify(messages)); return original(messages, options) }
const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, builtinTools: [], builtinPlugins: ['environment'], contextManager: null })
await agent.invoke('go')
const text = captured[0].content.map((b: any) => b.text ?? JSON.stringify(b)).join('\u0001')
console.log(JSON.stringify({ len: text.length, head: text.slice(0, 200), tail: text.slice(-900) }))
process.exit(0)
