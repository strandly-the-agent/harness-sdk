import { describe, it, expect } from 'vitest'
import { mergeModelInfo, type ModelInfo } from '../model-info.js'

const base: ModelInfo = {
  capabilities: { stateful: false, promptCaching: 'cachePoints' },
  limits: { contextWindow: 200_000 },
}

describe('mergeModelInfo', () => {
  it('returns the base when no patch is given', () => {
    expect(mergeModelInfo(base)).toEqual(base)
  })

  it('inherits fields a patch leaves out', () => {
    const merged = mergeModelInfo(base, { limits: { contextWindow: 1_000_000 } })

    expect(merged.capabilities).toEqual(base.capabilities)
    expect(merged.limits.contextWindow).toBe(1_000_000)
  })

  it('resets a field to unknown on an explicit null', () => {
    const merged = mergeModelInfo(base, { capabilities: { promptCaching: null } })

    expect(merged.capabilities.promptCaching).toBeUndefined()
    expect(merged.capabilities.stateful).toBe(false)
  })

  it('lets a later patch win over an earlier one', () => {
    const merged = mergeModelInfo(base, { capabilities: { stateful: true } }, undefined, {
      capabilities: { stateful: false, promptCaching: 'unsupported' },
    })

    expect(merged.capabilities).toEqual({ stateful: false, promptCaching: 'unsupported' })
  })

  it('returns a frozen record', () => {
    const merged = mergeModelInfo(base)

    expect(Object.isFrozen(merged)).toBe(true)
    expect(Object.isFrozen(merged.capabilities)).toBe(true)
    expect(Object.isFrozen(merged.limits)).toBe(true)
  })
})
