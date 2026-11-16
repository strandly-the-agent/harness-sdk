import { readFileSync } from 'node:fs'
import { z } from 'zod'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
import { tool } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/index.node.js'
const { options: optionSets, inputs } = JSON.parse(readFileSync('./scases.json', 'utf8'))
const DROP = new Set(['trackingId', 'metadata'])
const sort = (v: unknown): unknown =>
  Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).filter(([k]) => !DROP.has(k)).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, x]) => [k, sort(x)])) : v
process.chdir('/tmp/review12_correctness/ts/cwd2')
for (let o = 0; o < optionSets.length; o++) {
  for (let i = 0; i < inputs.length; i++) {
    if (o > 0 && i > 8) continue
    const id = `${o}.${i}`
    const model = new MockMessageModel()
    model.addTurn({ type: 'textBlock', text: 'first answer' })
    model.addTurn([{ type: 'toolUseBlock', name: 'my_tool', toolUseId: 'tu-my', input: {} }])
    model.addTurn([{ type: 'toolUseBlock', name: 'subagent', toolUseId: 'tu-sub', input: inputs[i] }])
    model.addTurn({ type: 'textBlock', text: 'child done' })
    model.addTurn({ type: 'textBlock', text: 'parent done' })
    model.addTurn({ type: 'textBlock', text: 'spare' })
    const captured: any[] = []
    const original = model.stream.bind(model)
    ;(model as any).stream = function (messages: any, options: any) {
      captured.push({ messages: JSON.parse(JSON.stringify(messages)), options })
      return original(messages, options)
    }
    const mine = tool({ name: 'my_tool', description: 'consumer', inputSchema: z.object({}), callback: () => 'ok' })
    try {
      const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, backgroundTasks: false, tools: [mine], ...optionSets[o] })
      await agent.invoke('first question')
      const result = await agent.invoke('go')
      console.log(`${id}\tresult ${result.stopReason} ${JSON.stringify(result.toString())} calls=${captured.length}`)
      captured.forEach((c, k) => {
        const sp = c.options.systemPrompt
        const spText = typeof sp === 'string' ? sp : JSON.stringify(sp)
        console.log(`${id}\treq${k} tools=${JSON.stringify((c.options.toolSpecs ?? []).map((s: any) => s.name))} sp=${spText.length}:${spText.slice(-60).replace(/\n/g, '\\n')}`)
        console.log(`${id}\treq${k} msgs=${JSON.stringify(sort(c.messages))}`)
      })
    } catch (e) {
      console.log(`${id}\terr ${(e as Error).message}`)
    }
  }
}
