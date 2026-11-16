import { mkdtempSync, readdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
process.chdir(mkdtempSync(join(tmpdir(), 'r12b-')))
const model = new MockMessageModel(); model.addTurn({ type: 'textBlock', text: 'done' })
try {
  const agent = await createHarness({ model: model as never, session: { id: 'a', dir: 'we\\ird' }, memory: false, skills: false, printer: false, builtinTools: [], builtinPlugins: [], contextManager: null })
  await agent.invoke('go')
  console.log('TS BACKSLASH OK ' + JSON.stringify(readdirSync('.')))
} catch (e) { console.log('TS BACKSLASH ERR ' + (e as Error).message) }
process.exit(0)
