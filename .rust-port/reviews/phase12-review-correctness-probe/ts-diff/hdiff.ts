import { readFileSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { z } from 'zod'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
import { tool } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/index.node.js'
const cases = JSON.parse(readFileSync('./hcases.json', 'utf8')) as Record<string, unknown>[]
const sort = (v: unknown): unknown =>
  Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, x]) => [k, sort(x)])) : v
const root = mkdtempSync(join(tmpdir(), 'r12ts-'))
process.chdir(root)
for (let i = 0; i < cases.length; i++) {
  const c = { ...cases[i] }
  const model = new MockMessageModel()
  model.addTurn({ type: 'textBlock', text: 'done' })
  let captured: any
  const original = model.stream.bind(model)
  ;(model as any).stream = function (messages: any, options: any) {
    captured ??= { messages, options }
    return original(messages, options)
  }
  const options: any = { model, session: false, memory: false, skills: false, printer: false, ...c }
  if (Array.isArray(c.tools)) {
    options.tools = (c.tools as string[]).map((name) => tool({ name, description: 'consumer', inputSchema: z.object({}), callback: () => 'ok' }))
  }
  try {
    const agent = await createHarness(options)
    await agent.invoke('go')
    const specs = (captured.options.toolSpecs ?? []) as any[]
    const sp = captured.options.systemPrompt
    const spText = typeof sp === 'string' ? sp : JSON.stringify(sp)
    console.log(`${i}\tok tools=${JSON.stringify(specs.map((s) => s.name))} sp=${spText.length}:${spText.slice(-40).replace(/\n/g, '\\n')} session=${JSON.stringify((agent as any).sessionManager?._sessionId ?? null)} name=${agent.name} desc=${(agent as any).description}`)
    for (const s of specs) console.log(`${i}\tspec ${s.name} ${JSON.stringify(sort(s.inputSchema))} :: ${String(s.description).length}`)
  } catch (e) {
    console.log(`${i}\terr ${(e as Error).message}`)
  }
}
