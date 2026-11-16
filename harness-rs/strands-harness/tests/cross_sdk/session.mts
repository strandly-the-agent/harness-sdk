/**
 * The TypeScript side of the cross-SDK session check; `main.rs` in this directory runs it.
 *
 * It runs in the session's base directory, so `createHarness` keeps the session under
 * `./.agent/sessions`, and loads harness-ts from the checkout `STRANDS_CROSS_SDK_HARNESS_TS`
 * names (dependencies installed).
 *
 * Usage: tsx session.mts <session id> [--expect <file>] [--prompt <text>] [--dump <file>]
 *
 * `--expect`: the restored history must equal the messages in the file. `--prompt`: run one
 * more turn. `--dump`: write the final history to the file.
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { isDeepStrictEqual } from 'node:util'

const harnessTs = process.env.STRANDS_CROSS_SDK_HARNESS_TS
if (!harnessTs) {
  throw new Error('STRANDS_CROSS_SDK_HARNESS_TS is not set')
}
const { createHarness } = await import(pathToFileURL(join(harnessTs, 'src/index.ts')).href)

const [id, ...rest] = process.argv.slice(2)
const flags: Record<string, string> = {}
for (let i = 0; i < rest.length; i += 2) {
  flags[rest[i]!] = rest[i + 1]!
}

const agent = await createHarness({
  model: 'bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0',
  effort: 'off',
  builtinTools: ['write', 'read'],
  builtinPlugins: [],
  skills: false,
  memory: false,
  session: { id },
})
await agent.initialize()

const history = (): unknown[] => JSON.parse(JSON.stringify(agent.messages.map((message: any) => message.toJSON())))

// Reports where two histories differ without printing message bodies.
function mismatch(actual: unknown[], expected: unknown[]): string | undefined {
  if (actual.length !== expected.length) {
    return `${actual.length} messages, expected ${expected.length}`
  }
  const index = actual.findIndex((message, i) => !isDeepStrictEqual(message, expected[i]))
  return index < 0 ? undefined : `message ${index} differs`
}

const restored = history()
if (flags['--expect']) {
  const expected = JSON.parse(readFileSync(flags['--expect'], 'utf8'))
  const difference = mismatch(restored, expected)
  if (difference) {
    throw new Error(`the restored history is not what the other SDK wrote: ${difference}`)
  }
}
if (flags['--prompt']) {
  const result = await agent.invoke(flags['--prompt'])
  if (result.stopReason !== 'endTurn') {
    throw new Error(`the turn stopped with ${result.stopReason}`)
  }
  const difference = mismatch(history().slice(0, restored.length), restored)
  if (difference) {
    throw new Error(`the turn changed the restored history: ${difference}`)
  }
}
if (flags['--dump']) {
  writeFileSync(flags['--dump'], JSON.stringify(history()))
}
await agent.shutdown()
