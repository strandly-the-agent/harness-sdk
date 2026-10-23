import { describe, it, expect } from 'vitest'
import { lookupModelInfo, getContextWindowLimit, lookupModel, stripRegionPrefix } from '../model-info.js'

describe('shared model info data', () => {
  it('strips one known region prefix', () => {
    expect(stripRegionPrefix('us.anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('us-gov.anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
  })

  it('resolves every spelling of a model to its one entry', () => {
    const entry = lookupModel('claude-opus-5-5')
    expect(entry?.limits?.contextWindow).toBe(1_000_000)
    expect(lookupModel('anthropic.claude-opus-5-5')).toBe(entry)
    expect(lookupModel('us.anthropic.claude-opus-5-5')).toBe(entry)
    expect(lookupModel('arn:aws:bedrock:us-west-2:123:inference-profile/global.anthropic.claude-opus-5-5')).toBe(entry)
  })

  it('resolves a dated snapshot to the undated model', () => {
    expect(lookupModel('anthropic.claude-opus-5-5-20260401-v1:0')).toBe(lookupModel('claude-opus-5-5'))
    expect(lookupModel('claude-haiku-4-5@20251001')).toBe(lookupModel('claude-haiku-4-5'))
    expect(lookupModel('claude-3-5-haiku-20241022')?.limits?.contextWindow).toBe(200_000)
  })

  it('does not read inherited object keys', () => {
    expect(lookupModel('toString')).toBeUndefined()
    expect(lookupModel('x.constructor')).toBeUndefined()
  })

  it('ignores a trailing Bedrock version suffix', () => {
    expect(lookupModel('anthropic.claude-3-5-haiku-20241022-v1:0')).toBe(lookupModel('claude-3-5-haiku-20241022'))
    expect(lookupModel('us.anthropic.claude-3-5-sonnet-20241022-v2:0')?.limits?.contextWindow).toBe(200_000)
  })

  it('returns undefined for unknown ids', () => {
    expect(lookupModel('unknown-model')).toBeUndefined()
    expect(lookupModel('')).toBeUndefined()
  })

  it('returns the entry facts and nothing for unknown ids', () => {
    expect(lookupModelInfo('eu.anthropic.claude-sonnet-4-6')).toEqual({
      capabilities: { promptCaching: 'cachePoints' },
      limits: { contextWindow: 1_000_000, maxOutputTokens: 128_000 },
    })
    expect(lookupModelInfo('anthropic.claude-opus-5-5').capabilities).toEqual({
      promptCaching: 'cachePoints',
      forcedToolChoice: false,
      nativeStructuredOutput: true,
    })
    expect(lookupModelInfo('amazon.nova-pro-v1:0')).toEqual({
      capabilities: { forcedToolChoice: true, nativeStructuredOutput: false },
      limits: { contextWindow: 300_000 },
    })
    expect(lookupModelInfo('claude-future-9')).toEqual({})
    expect(lookupModelInfo('')).toEqual({})
  })

  it('keeps getContextWindowLimit working for ids the Python table already knew', () => {
    expect(getContextWindowLimit('mistral-large-latest')).toBe(262_144)
    expect(getContextWindowLimit('o3')).toBe(200_000)
  })
})
