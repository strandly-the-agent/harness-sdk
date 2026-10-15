/**
 * Shared constants and helpers for the site-wide Python/TypeScript/Rust
 * language preference and clipboard interactions used by the landing page and
 * header.
 */

/**
 * localStorage key for the selected language. It is the canonical key for
 * every language tab group, whatever languages that group offers: AutoSyncTabs
 * always hands Starlight the synced-tabs hash of "Python|TypeScript"
 * (djb2 -> "jarkqt") for a language group, so adding a third tab does not
 * strand a reader's stored preference. It is referenced from both processed
 * scripts (via import) and inline scripts (via define:vars), so keep it here
 * as the single source of truth. If Starlight changes its hash format or
 * AutoSyncTabs changes its hash function, update this one place.
 */
export const LANGUAGE_STORAGE_KEY = 'starlight-synced-tabs__jarkqt'

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
