#!/usr/bin/env node
// Copies the shared model info (model-info/models.json at the repository root) into this package so it
// compiles and ships with it. Runs on install and before build; the copy is not committed.
import { copyFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const source = join(here, '../../model-info/models.json')
const target = join(here, '../src/models/model-info.json')

mkdirSync(dirname(target), { recursive: true })
copyFileSync(source, target)
