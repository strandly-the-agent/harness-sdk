import { describe, expect, it } from 'vitest'
import { hashTabLabels, LANGUAGE_STORAGE_KEY, LANGUAGE_SYNC_KEY } from '../src/util/language-preference'

describe('language preference storage key', () => {
  it('keeps the historical Python|TypeScript sync key so stored preferences survive', () => {
    expect(LANGUAGE_SYNC_KEY).toBe('jarkqt')
    expect(LANGUAGE_STORAGE_KEY).toBe('starlight-synced-tabs__jarkqt')
  })

  it('hashes labels the way AutoSyncTabs always has', () => {
    expect(hashTabLabels('Python|TypeScript')).toBe('jarkqt')
    expect(hashTabLabels('npm|yarn')).not.toBe(hashTabLabels('yarn|npm'))
  })
})
