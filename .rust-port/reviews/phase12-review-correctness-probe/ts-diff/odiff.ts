import { readFileSync } from 'node:fs'
import { harnessAgentOptionsFromConfig, defineHarnessAgentConfig } from './harness-ts/src/config.js'
process.env.HOME = '/home/u'
delete process.env.UNSET_VAR_XYZ
const cases = JSON.parse(readFileSync('./ocases.json', 'utf8')) as unknown[]
const sort = (v: unknown): unknown =>
  Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, x]) => [k, sort(x)])) : v
for (let i = 0; i < cases.length; i++) {
  try {
    const options = await harnessAgentOptionsFromConfig(defineHarnessAgentConfig(cases[i] as never), '/tmp/review12_correctness/ts/base')
    console.log(`${i}\t` + JSON.stringify({ ok: sort(options) }))
  } catch (e) {
    console.log(`${i}\t` + JSON.stringify({ err: (e as Error).message }))
  }
}
