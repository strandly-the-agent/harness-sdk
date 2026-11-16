import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
import { readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
process.chdir('/tmp/review12_correctness/ts/cwd3')
const model = new MockMessageModel()
model.addTurn({ type: 'textBlock', text: 'noted' })
const agent = await createHarness({ model: model as never, session: { id: 'checkpoint', dir: '.' }, memory: false, skills: false, printer: false, builtinTools: [], builtinPlugins: [], contextManager: null })
await agent.invoke('Remember the checkpoint')
const walk = (d: string): string[] => readdirSync(d).flatMap((n) => (statSync(join(d, n)).isDirectory() ? walk(join(d, n)) : [join(d, n)]))
console.log(JSON.stringify(walk('.')))
