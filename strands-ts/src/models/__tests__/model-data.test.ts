import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect } from 'vitest'
import {
  declaredModelInfo,
  getContextWindowLimit,
  lookupFamily,
  lookupModel,
  stripRegionPrefix,
} from '../model-data.js'

const here = dirname(fileURLToPath(import.meta.url))

describe('shared model data', () => {
  it('matches model-data/models.json at the repository root', () => {
    const source = readFileSync(join(here, '../../../../model-data/models.json'), 'utf8')
    const copy = readFileSync(join(here, '../data/models.json'), 'utf8')

    expect(copy).toBe(source)
  })

  it('strips one known region prefix', () => {
    expect(stripRegionPrefix('us.anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('us-gov.anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('anthropic.claude-sonnet-4-6')).toBe('anthropic.claude-sonnet-4-6')
    expect(stripRegionPrefix('arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6')).toBe(
      'arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6'
    )
  })

  it('looks up a model with or without its region prefix', () => {
    expect(lookupModel('global.anthropic.claude-sonnet-4-6')?.limits?.context_window).toBe(1_000_000)
    expect(lookupModel('anthropic.claude-sonnet-4-6')?.limits?.context_window).toBe(1_000_000)
    expect(lookupModel('unknown-model')).toBeUndefined()
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
