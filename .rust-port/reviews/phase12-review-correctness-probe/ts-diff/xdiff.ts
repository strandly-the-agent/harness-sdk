import { readFileSync } from 'node:fs'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
import { Agent } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/index.node.js'
import { makeSubagent, Choice, Option, Fixed, Inherit, Open, Preset } from './harness-ts/src/tools/subagent.js'
const { configs, inputs } = JSON.parse(readFileSync('./xcases.json', 'utf8'))
const sort = (v: unknown): unknown =>
  Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, x]) => [k, sort(x)])) : v
const choice = (a: any) => new Choice(a.choice.map((o: any) => (typeof o === 'string' ? o : new Option(o.name, { ...(o.value !== undefined && { value: o.value }), ...(o.description !== undefined && { description: o.description }) }))), a.multiple ?? false)
const axis = (a: any) => (a === undefined ? undefined : a.choice ? choice(a) : 'fixed' in a ? new Fixed(a.fixed) : a.inherit ? new Inherit() : new Open())
for (let c = 0; c < configs.length; c++) {
  const cfg = configs[c]
  for (let i = 0; i < inputs.length; i++) {
    const id = `${c}.${i}`
    let spec: any = null
    try {
      const options: any = {
        builder: (s: any) => { spec = s; const m = new MockMessageModel(); m.addTurn({ type: 'textBlock', text: 'child done' }); return new Agent({ model: m, printer: false }) },
        ...(cfg.presets && { presets: Object.fromEntries(Object.entries(cfg.presets).map(([k, v]: any) => [k, new Preset(v)])) }),
        ...(cfg.defaultPreset !== undefined && { defaultPreset: cfg.defaultPreset }),
        ...(cfg.instructions && { instructions: axis(cfg.instructions) }),
        ...(cfg.tools && { tools: axis(cfg.tools) }),
        ...(cfg.mcpServers && { mcpServers: axis(cfg.mcpServers) }),
        ...(cfg.model && { model: axis(cfg.model) }),
        ...(cfg.context && { context: axis(cfg.context) }),
        ...(cfg.inheritedTools && { inheritedTools: cfg.inheritedTools }),
        ...(cfg.inheritedMcpServers && { inheritedMcpServers: cfg.inheritedMcpServers }),
        ...(cfg.maxDepth !== undefined && { maxDepth: cfg.maxDepth }),
        ...(cfg.name && { name: cfg.name }),
      }
      const tool = makeSubagent(options)
      if (i === 0) console.log(`${c}.schema\t${tool.name}\t${JSON.stringify(sort(tool.toolSpec.inputSchema))}\t${JSON.stringify(tool.toolSpec.description.slice(300))}`)
      const model = new MockMessageModel()
      model.addTurn([{ type: 'toolUseBlock', name: tool.name, toolUseId: 't1', input: inputs[i] }])
      model.addTurn({ type: 'textBlock', text: 'done' })
      const agent = new Agent({ model, tools: [tool], printer: false })
      await agent.invoke('go')
      const result = (JSON.parse(JSON.stringify(agent.messages)) as any[]).flatMap((m) => m.content).find((b: any) => b.toolResult)?.toolResult
      const text = result.content.map((x: any) => x.text ?? JSON.stringify(x)).join('|')
      const s = spec === null ? null : { task: spec.task, agentType: spec.agentType, instructions: spec.instructions, tools: spec.tools, mcpServers: spec.mcpServers, model: spec.model, context: spec.context, lastMessages: spec.lastMessages }
      console.log(`${id}\t${result.status}\t${JSON.stringify(text)}\t${JSON.stringify(s)}`)
    } catch (e) {
      console.log(`${id}\terr ${(e as Error).message}`)
      if (i === 0) break
    }
  }
}
process.exit(0)
