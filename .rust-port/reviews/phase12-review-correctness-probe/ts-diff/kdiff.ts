import { readFileSync, readdirSync, statSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
const cases = JSON.parse(readFileSync('./kcases.json', 'utf8')) as any[]
process.chdir('/tmp/review12_correctness/ts/cwd4')
const walk = (d: string): string[] => readdirSync(d).flatMap((n) => (statSync(join(d, n)).isDirectory() ? walk(join(d, n)) : [join(d, n)]))
for (let k = 0; k < cases.length; k++) {
  const c = cases[k]
  for (const d of ['sess', '.agent/sessions', '.agent/memory']) rmSync(d, { recursive: true, force: true })
  const model = new MockMessageModel()
  if (c.activate) model.addTurn([{ type: 'toolUseBlock', name: 'skills', toolUseId: 'k1', input: { skill_name: c.activate } }])
  model.addTurn({ type: 'textBlock', text: 'done' })
  const captured: any[] = []
  const original = model.stream.bind(model)
  ;(model as any).stream = function (messages: any, options: any) {
    captured.push({ messages: JSON.parse(JSON.stringify(messages)), options })
    return original(messages, options)
  }
  try {
    const agent = await createHarness({ model: model as never, session: c.session ?? false, memory: c.memory ?? false, skills: c.skills, printer: false, backgroundTasks: false, builtinTools: [], builtinPlugins: [], contextManager: null })
    await agent.invoke('go')
    captured.forEach((r, i) => {
      const sp = r.options.systemPrompt
      const spText = typeof sp === 'string' ? sp : JSON.stringify(sp)
      console.log(`${k}\treq${i} tools=${JSON.stringify((r.options.toolSpecs ?? []).map((s: any) => s.name))} sp=${spText.length}:${JSON.stringify(spText.slice(1665))}`)
      const last = r.messages[r.messages.length - 1]
      console.log(`${k}\treq${i} last=${JSON.stringify(last.content).slice(0, 600)}`)
    })
    const files = walk('.').filter((f) => !f.startsWith('other') && !f.startsWith('.agent/skills')).map((f) => f.replace(/[0-9a-f]{8}(?=\/scopes)/, '<id>')).sort()
    console.log(`${k}\tfiles=${JSON.stringify(files)} session=${(agent as any).sessionManager ? 'yes' : 'no'}`)
  } catch (e) {
    console.log(`${k}\terr ${(e as Error).message}`)
  }
}
process.exit(0)
