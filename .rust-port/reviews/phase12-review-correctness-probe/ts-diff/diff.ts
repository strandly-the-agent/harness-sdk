import { readFileSync } from 'node:fs'
import { defineHarnessAgentConfig } from './harness-ts/src/config.js'
const cases = JSON.parse(readFileSync('./cases.json', 'utf8')) as unknown[]
const sort = (v: unknown): unknown =>
  Array.isArray(v) ? v.map(sort) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, x]) => [k, sort(x)])) : v
const orig = console.warn
cases.forEach((c, i) => {
  try {
    console.log(`${i}\t` + JSON.stringify({ ok: sort(defineHarnessAgentConfig(c as never)) }))
  } catch (e) {
    console.log(`${i}\t` + JSON.stringify({ err: (e as Error).message }))
  }
})
