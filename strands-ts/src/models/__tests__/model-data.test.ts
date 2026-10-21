import { describe, it, expect } from 'vitest'
import {
  declaredModelInfo,
  getContextWindowLimit,
  lookupFamily,
  lookupModel,
  stripRegionPrefix,
} from '../model-data.js'

describe('shared model data', () => {
  it('strips one known region prefix', () => {
    expect(stripRegionPrefix('us.anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('us-gov.anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6')).toBe(
      'arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6'
    )
  })

  it('looks up a model by base id after stripping leading segments, like the Python SDK', () => {
    expect(lookupModel('global.anthropic.claude-sonnet-4-6')?.limits?.context_window).toBe(1_000_000)
    expect(lookupModel('anthropic.claude-sonnet-4-6')?.limits?.context_window).toBe(1_000_000)
    expect(
      lookupModel('arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6')?.limits
        ?.context_window
    ).toBe(1_000_000)
    expect(lookupModel('unknown-model')).toBeUndefined()
    expect(lookupModel('')).toBeUndefined()
  })

  it('resolves the family from the id when the model has no entry', () => {
    expect(lookupFamily('us.anthropic.claude-future-9')?.match).toContain('anthropic')
    expect(lookupFamily('amazon.nova-pro-v1:0')).toBeUndefined()
  })

  it('declares cache points for the Claude family and nothing for unknown ids', () => {
    expect(declaredModelInfo('eu.anthropic.claude-sonnet-4-6')).toEqual({
      capabilities: { promptCaching: 'cachePoints' },
    })
    expect(declaredModelInfo('claude-opus-4-6')).toEqual({ capabilities: { promptCaching: 'cachePoints' } })
    expect(declaredModelInfo('amazon.nova-pro-v1:0')).toEqual({})
    expect(declaredModelInfo('')).toEqual({})
  })

  it('keeps getContextWindowLimit working for ids the Python table already knew', () => {
    expect(getContextWindowLimit('mistral-large-latest')).toBe(262_144)
    expect(getContextWindowLimit('o3')).toBe(200_000)
  })
})
