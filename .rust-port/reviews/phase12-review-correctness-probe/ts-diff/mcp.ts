import { readFileSync, rmSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createHarness } from './harness-ts/src/agent.js'
import { MockMessageModel } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/__fixtures__/mock-message-model.js'
const LOG = '/tmp/review12_correctness/mcp/log.txt'
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const tu = (name: string, id: string, input: unknown) => [{ type: 'toolUseBlock', name, toolUseId: id, input }]
const text = (t: string) => ({ type: 'textBlock', text: t })
const one = { srv: { command: 'python3', args: ['/tmp/review12_correctness/mcp/server.py'] } }
async function run(label: string, turns: any[], servers: any) {
  process.chdir(mkdtempSync(join(tmpdir(), 'r12mcp-')))
  rmSync(LOG, { force: true })
  const model = new MockMessageModel()
  turns.forEach((t) => model.addTurn(t))
  const captured: string[][] = []
  const original = model.stream.bind(model)
  ;(model as any).stream = async function* (messages: any, options: any) {
    captured.push((options.toolSpecs ?? []).map((s: any) => s.name).filter((n: string) => n.includes('srv') || n === 'subagent'))
    await sleep(300)
    yield* original(messages, options)
  }
  const agent = await createHarness({ model: model as never, session: false, memory: false, skills: false, printer: false, backgroundTasks: false, mcpServers: servers })
  const result = await agent.invoke('go')
  console.log(`[${label}] result ${result.stopReason} ${result.toString()}`)
  captured.forEach((names, k) => console.log(`[${label}] req${k} tools=${JSON.stringify(names)}`))
  await sleep(300)
  console.log(`[${label}] server log:\n${readFileSync(LOG, 'utf8')}`)
}
await run('control', [tu('srv_mutate', 'm1', {}), tu('srv_echo', 'e1', { text: 'a' }), tu('srv_echo', 'e2', { text: 'b' }), text('done')], one)
await run('delegated', [tu('subagent', 's1', { task: 't' }), text('child done'), tu('srv_mutate', 'm1', {}), tu('srv_echo', 'e1', { text: 'a' }), tu('srv_echo', 'e2', { text: 'b' }), text('done')], one)
process.exit(0)
