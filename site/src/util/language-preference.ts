/**
 * Shared constants and helpers for the site-wide Python/TypeScript/Rust
 * language preference and clipboard interactions used by the landing page and
 * header.
 */

/**
 * Hash used by AutoSyncTabs to derive a Starlight `syncKey` from tab labels
 * (Java `String.hashCode`, base36).
 */
export function hashTabLabels(str: string): string {
  let hash = 0
  for (let i = 0; i < str.length; i++) {
    hash = (hash << 5) - hash + str.charCodeAt(i)
    hash = hash & hash
  }
  return Math.abs(hash).toString(36)
}

/**
 * Canonical syncKey for every language tab group, whatever languages the group
 * offers: the hash of the original "Python|TypeScript" pair ("jarkqt"), so
 * adding a Rust tab does not strand a reader's stored preference.
 */
export const LANGUAGE_SYNC_KEY = hashTabLabels('Python|TypeScript')

/**
 * localStorage key for the selected language (Starlight prefixes the syncKey).
 * Referenced from processed scripts (via import) and inline scripts (via
 * define:vars) — this is the single source of truth.
 */
export const LANGUAGE_STORAGE_KEY = `starlight-synced-tabs__${LANGUAGE_SYNC_KEY}`

/**
 * Language assumed when no preference is stored yet. Label form ("TypeScript"/
 * "Python"/"Rust") because that is what Starlight's synced tabs write to
 * storage.
 */
export const DEFAULT_LANGUAGE_LABEL = 'TypeScript'

/**
 * Copy text to the clipboard, resolving to whether it succeeded. Returns false
 * instead of throwing when the Clipboard API is unavailable (non-secure
 * contexts) or permission is denied, so callers can avoid showing a success
 * state for a copy that did not happen.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}
