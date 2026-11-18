/**
 * HTML → markdown extraction for the web fetch tool.
 *
 * Delegates HTML-to-markdown conversion to turndown with ATX headings and
 * fenced code blocks. Non-content elements (scripts, styles, media, forms,
 * etc.) are dropped entirely. `data:` URI images are replaced with their alt
 * text (or nothing) to avoid enormous blobs in the output. `javascript:` and
 * `vbscript:` image sources are dropped, and `javascript:`, `vbscript:`, and
 * `data:` links are reduced to their text. Image alt text and sources are
 * escaped so they cannot change the structure of the output.
 *
 * Requires `turndown` as an optional peer dependency.
 */

import type TurndownService from 'turndown'

import { logger } from '../../logging/logger.js'

// Elements whose entire subtree is discarded.
const DROPPED_ELEMENTS = [
  'script',
  'style',
  'noscript',
  'template',
  'svg',
  'canvas',
  'iframe',
  'object',
  'embed',
  'video',
  'audio',
  'form',
  'input',
  'button',
  'select',
  'textarea',
  'nav',
]

// Strip whitespace and invisible characters before URL scheme detection.
function _stripInvisible(value: string): string {
  return value.replace(/^[\p{Cc}\p{Cf}\p{Zs}\p{Zl}\p{Zp}]+/u, '')
}

// Strip what URL parsers strip from both ends of a URL. trim() differs from Python's strip().
function _trimC0ControlOrSpace(value: string): string {
  let start = 0
  let end = value.length
  while (start < end && value.charCodeAt(start) <= 0x20) start++
  while (end > start && value.charCodeAt(end - 1) <= 0x20) end--
  return value.slice(start, end)
}

// Collapse whitespace and control characters, and escape markdown and HTML syntax, so image text is plain text.
function _escapeImageText(text: string): string {
  return text
    .replace(/[\s\p{Cc}]+/gu, ' ')
    .replace(/[\\[\](`~]|^\^/g, '\\$&')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

// Drop tabs and newlines as URL parsers do, and percent-encode what a markdown parser could read as syntax.
function _escapeImageSrc(src: string): string {
  return src
    .replace(/[\t\n\r]/g, '')
    .replace(/[\s\p{Cc}<>\\"`]/gu, (char) => encodeURIComponent(char))
    .replace(/[()']/g, (char) => `%${char.charCodeAt(0).toString(16)}`)
}

async function loadTurndown(): Promise<typeof TurndownService> {
  try {
    return (await import('turndown')).default
  } catch (cause) {
    throw new Error("web_fetch requires the 'turndown' package. Install it with: npm install turndown", { cause })
  }
}

async function _createTurndownService(): Promise<TurndownService> {
  const Turndown = await loadTurndown()

  const td = new Turndown({
    headingStyle: 'atx',
    codeBlockStyle: 'fenced',
    bulletListMarker: '-',
    emDelimiter: '*',
    strongDelimiter: '**',
    linkStyle: 'inlined',
  })

  // Cast needed because svg/noscript/template are absent from HTMLElementTagNameMap.
  td.remove(DROPPED_ELEMENTS as unknown as import('turndown').Filter)

  // Render the <title> as a top-level heading so it appears in the document
  // body rather than being silently dropped (turndown ignores <head> elements).
  td.addRule('title', {
    filter: 'title',
    replacement: (_content, node) => {
      const text = (node as HTMLElement).textContent?.trim() ?? ''
      return text ? `# ${text}\n\n` : ''
    },
  })

  // Replace data: URI images with alt text only (they can be enormous blobs).
  // Strip javascript: and vbscript: src values entirely.
  td.addRule('safeImage', {
    filter: 'img',
    replacement: (_content, node) => {
      const element = node as HTMLImageElement
      const src = _trimC0ControlOrSpace(element.getAttribute('src') ?? '')
      const alt = _escapeImageText(element.getAttribute('alt') ?? '')
      if (!src) return alt
      try {
        const scheme = new URL(_stripInvisible(src)).protocol
        if (scheme === 'javascript:' || scheme === 'vbscript:') return ''
        if (scheme === 'data:') return alt
      } catch {
        // not an absolute URL — keep the image
      }
      return `![${alt}](${_escapeImageSrc(src)})`
    },
  })

  // Render only the link text for javascript:, vbscript:, and data: hrefs.
  td.addRule('safeLink', {
    filter: (node) => {
      if (node.nodeName !== 'A') return false
      const href = ((node as HTMLAnchorElement).getAttribute('href') ?? '').trim()
      try {
        const scheme = new URL(_stripInvisible(href)).protocol
        return scheme === 'javascript:' || scheme === 'vbscript:' || scheme === 'data:'
      } catch {
        return false
      }
    },
    replacement: (content) => content,
  })

  return td
}

// Cache the configured service so rules are registered once across all calls.
let _turndownServicePromise: Promise<TurndownService> | null = null

/**
 * Convert HTML to markdown suitable for a model to read.
 *
 * The page title, if present, is rendered as a top-level ATX heading so the
 * model sees it as part of the document. Returns the original HTML string if
 * conversion fails.
 *
 * @param html - Raw HTML string to convert.
 * @returns The converted markdown string, or the original HTML if conversion failed.
 */
export async function htmlToMarkdown(html: string): Promise<string> {
  _turndownServicePromise ??= _createTurndownService()
  const td = await _turndownServicePromise
  try {
    const markdown = td.turndown(html).trim()
    return markdown ? markdown + '\n' : ''
  } catch (error) {
    logger.warn(`error=<${error}> | html_to_markdown failed, returning raw html`)
    return html
  }
}
