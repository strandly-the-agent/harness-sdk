#!/usr/bin/env node
// Copies model-info/models.json into each package that ships it. `--check` fails instead of writing.
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const source = readFileSync(join(root, 'model-info/models.json'), 'utf8')
const targets = ['strands-ts/src/models/model-info.json']
const check = process.argv.includes('--check')

let stale = 0
for (const target of targets) {
  const path = join(root, target)
  let current
  try {
    current = readFileSync(path, 'utf8')
  } catch {
    current = undefined
  }
  if (current === source) continue
  if (check) {
    console.error(`${target} differs from model-info/models.json; run \`npm run model-info:generate\``)
    stale++
  } else {
    writeFileSync(path, source)
    console.log(`wrote ${target}`)
  }
}
process.exit(stale === 0 ? 0 : 1)
