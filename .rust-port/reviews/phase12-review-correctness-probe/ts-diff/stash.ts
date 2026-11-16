import { readdirSync, statSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
process.chdir('/tmp/review12_correctness/ts/cwd5')
const walk = (d: string): string[] => { try { return readdirSync(d).flatMap((n) => (statSync(join(d, n)).isDirectory() ? walk(join(d, n)) : [join(d, n)])) } catch { return [] } }
for (const [k, session] of [[0, { id: 's3' }], [1, false], [2, { id: 's4', dir: 'custom/dir' }]] as const) {
  for (const d of ['.agent', 'custom']) rmSync(d, { recursive: true, force: true })
  const model = new MockMessageModel()
  model.addTurn([{ type: 'toolUseBlock', name: 'shell', toolUseId: 'big1', input: { command: 'seq 1 60000' } }])
  model.addTurn({ type: 'textBlock', text: 'done' })
  const captured: any[] = []
  const original = model.stream.bind(model)
  ;(model as any).stream = function (messages: any, options: any) {
    captured.push(JSON.parse(JSON.stringify(messages)))
    return original(messages, options)
  }
  const agent = await createHarness({ model: model as never, session: session as never, memory: false, skills: false, printer: false, backgroundTasks: false, builtinPlugins: [] })
  await agent.invoke('go')
  const last = captured[captured.length - 1]
  const result = last[last.length - 1].content.map((b: any) => (b.toolResult ? b.toolResult.content.map((c: any) => c.text ?? JSON.stringify(c)).join('|') : JSON.stringify(b))).join('||')
  console.log(`${k}\tresult len=${result.length} head=${JSON.stringify(result.slice(0, 160))} tail=${JSON.stringify(result.slice(-330))}`)
  console.log(`${k}\tfiles=${JSON.stringify(walk('.').sort())}`)
}
process.exit(0)
