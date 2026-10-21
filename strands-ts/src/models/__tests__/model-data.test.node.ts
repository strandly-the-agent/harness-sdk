import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))

describe('shared model data copy', () => {
  it('matches model-data/models.json at the repository root', () => {
    const source = readFileSync(join(here, '../../../../model-data/models.json'), 'utf8')
    const copy = readFileSync(join(here, '../data/models.json'), 'utf8')

    expect(copy).toBe(source)
  })
})
