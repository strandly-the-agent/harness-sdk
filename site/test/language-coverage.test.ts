import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import fg from 'fast-glob'
import { describe, expect, it } from 'vitest'
import { ALL_SDK_LANGUAGES } from '../src/config/languages'

/**
 * Enforces the "Language coverage" rule from .agents/references/mdx-authoring.md:
 * on a page, every language tab group must offer every language the page offers
 * (frontmatter `languages`, default all SDKs), and on a page that offers Rust every
 * <Syntax> needs an `rs` prop. Without this a Rust reader silently sees another
 * language's panel or identifier.
 */
const DOCS_ROOT = join(__dirname, '../src/content/docs')
const LABELS: Record<string, string> = { python: 'Python', typescript: 'TypeScript', rust: 'Rust' }
const LANGUAGE_LABELS = new Set(Object.values(LABELS))

function pageLanguages(source: string): string[] {
  const fm = source.match(/^---\n([\s\S]*?)\n---/)
  const line = fm?.[1].split('\n').find((l) => l.startsWith('languages:'))
  if (!line) return [...ALL_SDK_LANGUAGES]
  const value = line.slice('languages:'.length).trim()
  const langs = value.startsWith('[')
    ? value.slice(1, -1).split(',')
    : value
      ? [value]
      : []
  return langs.map((l) => l.trim().toLowerCase()).filter(Boolean)
}

/** Labels of every <Tabs> group, in document order (nested groups handled with a stack). */
function tabGroups(source: string): { line: number; labels: string[] }[] {
  const groups: { line: number; labels: string[] }[] = []
  const stack: { line: number; labels: string[] }[] = []
  source.split('\n').forEach((text, i) => {
    const t = text.trim()
    if (t === '<Tabs>' || t.startsWith('<Tabs ')) stack.push({ line: i + 1, labels: [] })
    else if (t === '</Tabs>' && stack.length) groups.push(stack.pop()!)
    else {
      const m = t.match(/^<Tab label="([^"]+)"/)
      if (m && stack.length) stack[stack.length - 1].labels.push(m[1])
    }
  })
  return groups
}

const pages = fg.sync('**/*.mdx', { cwd: DOCS_ROOT, ignore: ['**/_generated/**'] })

describe('language coverage', () => {
  it('finds docs pages', () => {
    expect(pages.length).toBeGreaterThan(100)
  })

  it('every language tab group offers every language the page offers', () => {
    const failures: string[] = []
    for (const page of pages) {
      const source = readFileSync(join(DOCS_ROOT, page), 'utf-8')
      const offered = pageLanguages(source).map((l) => LABELS[l] ?? l)
      for (const group of tabGroups(source)) {
        const langs = group.labels.filter((l) => LANGUAGE_LABELS.has(l))
        if (langs.length < 2) continue
        const missing = offered.filter((l) => !langs.includes(l))
        if (missing.length) failures.push(`${page}:${group.line} lacks ${missing.join(', ')} tab`)
      }
    }
    expect(failures).toEqual([])
  })

  it('every <Syntax> on a page that offers Rust has an rs prop', () => {
    const failures: string[] = []
    for (const page of pages) {
      const source = readFileSync(join(DOCS_ROOT, page), 'utf-8')
      if (!pageLanguages(source).includes('rust')) continue
      source.split('\n').forEach((text, i) => {
        for (const tag of text.match(/<Syntax\b[^>]*\/>/g) ?? []) {
          if (!/\brs=/.test(tag)) failures.push(`${page}:${i + 1} ${tag}`)
        }
      })
    }
    expect(failures).toEqual([])
  })
})

