import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))

describe('shared model info copy', () => {
  it('matches model-info/models.json at the repository root', () => {
    const source = readFileSync(join(here, '../../../../model-info/models.json'), 'utf8')
    const copy = readFileSync(join(here, '../model-info.json'), 'utf8')

    expect(copy).toBe(source)
  })
})
