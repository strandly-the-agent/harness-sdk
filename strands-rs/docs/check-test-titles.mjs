// Checks that every TS test case of an in-scope PARITY.md row has a Rust test named after it.
//
// Usage: node strands-rs/docs/check-test-titles.mjs [--root <repo>] [--ts <spec root>] [--report] [filter...]
//   --root    repository root (default: two directories above this script)
//   --ts      TS source root the PARITY.md spec paths point into (default: <root>/strands-ts/src)
//   --report  also print each missing title with the Rust name it expects
//   filter    only check rows whose spec path contains one of these substrings
// Needs the `typescript` package: run it from a checkout with `node_modules`, or set `NODE_PATH`.
//
// For each row of the "In scope" table in strands-rs/docs/PARITY.md it lists the spec's cases (the calls
// count-ts-cases.mjs counts; an `it.each` case counts once, by its template text) and sorts each into:
//   matched   a Rust test function has the snake_case name of the title
//   declared  test-title-map.json names the Rust test that covers the case
//   gap       test-title-map.json gives a reason, or the row ports nothing (`0/N`) and gives one
//   missing   none of the above
// It exits 1 when a case is missing, when matched + declared differs from the row's `ported` count, or
// when the cases found differ from the row's `total`.
//
// snake_case rule: lowercase the title; every run of characters outside `a-z0-9` becomes one `_`; trim
// `_` from both ends; prefix `_` when the result starts with a digit or is empty. A template-literal
// title uses its source text, so `${name}` contributes `name`. No truncation: names are exact.
//
// Rust tests: every `fn <name>(` that follows a test attribute (`#[test]`, `#[tokio::test]`,
// `#[rstest]`, `#[test_case(..)]`, any `#[<path>::test]`) in the files the row's second column names.
// `crate::a::b` resolves to the longest existing prefix (`a/b.rs`, `a/b/mod.rs`, and everything under
// `a/b/` when the whole path resolves); a file or directory path is taken as is; a trybuild case file
// counts as a test named after its stem. A row that names no file is checked against all of
// `strands-rs/`. A title that repeats in a spec (under different `describe` blocks) needs as many Rust
// tests with that name, each in its own `mod`.
//
// test-title-map.json: { "<spec path>": { "<ts title>": "<rust test name>" | { "gap": "<reason>" } } }
// The key is the title as written, or the `describe` titles and the title joined with " > " when the
// bare title repeats. The key "*" applies to every title of the spec that nothing else covers. A Rust
// test name is the function name, optionally with leading `mod::` segments; it must exist in strands-rs.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

// `require` rather than `import`, so that `NODE_PATH` can supply the package.
const ts = createRequire(import.meta.url)('typescript')

const ROOTS = new Set(['it', 'test'])
const FACTORIES = new Set(['each', 'for', 'skipIf', 'runIf'])
const CRATES = { strands: 'strands-rs/strands/src', strands_macros: 'strands-rs/strands-macros/src' }
const TEST_ATTRIBUTE = /^\s*#\[\s*(?:\w+::)*(?:test|rstest|test_case)\b/
const FUNCTION = /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(?:r#)?(\w+)\s*[(<]/

const args = process.argv.slice(2)
const option = (name) => {
  const at = args.indexOf(name)
  return at < 0 ? undefined : args.splice(at, 2)[1]
}
const root = resolve(option('--root') ?? join(dirname(fileURLToPath(import.meta.url)), '..', '..'))
const tsRoot = resolve(option('--ts') ?? join(root, 'strands-ts', 'src'))
const report = args.includes('--report')
const filters = args.filter((arg) => arg !== '--report')

export function snakeCase(title) {
  const name = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
  return /^[a-z]/.test(name) ? name : `_${name}`
}

function rootName(node) {
  while (true) {
    if (ts.isIdentifier(node)) return node.text
    if (ts.isPropertyAccessExpression(node)) node = node.expression
    else if (ts.isCallExpression(node)) node = node.expression
    else return undefined
  }
}

function isCall(call, roots) {
  const callee = call.expression
  if (ts.isPropertyAccessExpression(callee) && FACTORIES.has(callee.name.text)) return false
  return roots.has(rootName(callee))
}

function titleOf(call, source) {
  const title = call.arguments[0]
  if (title === undefined) return ''
  if (ts.isStringLiteralLike(title)) return title.text
  if (ts.isTemplateExpression(title)) return title.getText(source).slice(1, -1)
  return title.getText(source)
}

// The cases of a spec file, each with its title and the titles of the enclosing `describe` blocks.
function cases(path) {
  const source = ts.createSourceFile(path, readFileSync(path, 'utf8'), ts.ScriptTarget.Latest, true)
  const found = []
  const visit = (node, describes) => {
    if (ts.isCallExpression(node)) {
      if (isCall(node, ROOTS)) {
        const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1
        found.push({ title: titleOf(node, source), describes, line })
      } else if (isCall(node, new Set(['describe']))) {
        describes = [...describes, titleOf(node, source)]
      }
    }
    ts.forEachChild(node, (child) => visit(child, describes))
  }
  visit(source, [])
  return found
}

function rustFiles(path) {
  if (!existsSync(path)) return []
  if (!statSync(path).isDirectory()) return path.endsWith('.rs') ? [path] : []
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) =>
    entry.name === 'target' || entry.name === 'node_modules' ? [] : rustFiles(join(path, entry.name))
  )
}

const testNamesCache = new Map()
function testNames(file) {
  if (!testNamesCache.has(file)) {
    const names = []
    let attribute = false
    for (const line of readFileSync(file, 'utf8').split('\n')) {
      if (TEST_ATTRIBUTE.test(line)) attribute = true
      const name = FUNCTION.exec(line)?.[1]
      if (name !== undefined) {
        if (attribute) names.push(name)
        attribute = false
      }
    }
    // A trybuild case is a whole file.
    if (/[\\/]tests[\\/]ui[\\/]/.test(file)) names.push(basename(file, '.rs'))
    testNamesCache.set(file, names)
  }
  return testNamesCache.get(file)
}

// `strands::a::{b, c}` -> `strands::a::b`, `strands::a::c`.
function expandBraces(token) {
  const group = /^(.*)\{([^{}]*)\}(.*)$/.exec(token)
  if (group === null) return [token]
  return group[2].split(',').flatMap((part) => expandBraces(`${group[1]}${part.trim()}${group[3]}`))
}

function filesOfToken(token) {
  const segments = token.split('::')
  const crate = CRATES[segments[0]]
  if (crate === undefined) {
    return [join(root, token), join(root, 'strands-rs', token)].flatMap(rustFiles)
  }
  for (let length = segments.length; length > 1; length--) {
    const base = join(root, crate, ...segments.slice(1, length))
    const files = [`${base}.rs`, join(base, 'mod.rs')].filter(existsSync)
    const whole = length === segments.length
    if (whole) files.push(...rustFiles(base).filter((file) => !files.includes(file)))
    if (files.length > 0) return files
  }
  return []
}

function filesOfRow(cell) {
  const tokens = [...cell.matchAll(/`([^`]+)`/g)].flatMap((match) => expandBraces(match[1]))
  return [...new Set(tokens.flatMap(filesOfToken))]
}

function rows() {
  const lines = readFileSync(join(root, 'strands-rs/docs/PARITY.md'), 'utf8').split('\n')
  const start = lines.indexOf('## In scope')
  const end = lines.findIndex((line, at) => at > start && line.startsWith('## '))
  return lines.slice(start, end < 0 ? undefined : end).flatMap((line) => {
    const cells = line.split('|').map((cell) => cell.trim())
    const spec = /^`(strands-ts\/src\/[^`]+)`$/.exec(cells[1] ?? '')?.[1]
    const counts = /^(\d+)\/(\d+)$/.exec(cells[3] ?? '')
    if (spec === undefined || counts === null) return []
    const gaps = cells.slice(4, -1).join('|').trim()
    return [{ spec, tests: cells[2], ported: Number(counts[1]), total: Number(counts[2]), gaps }]
  })
}

const mapPath = join(root, 'strands-rs/docs/test-title-map.json')
const titleMap = existsSync(mapPath) ? JSON.parse(readFileSync(mapPath, 'utf8')) : {}
const allFiles = rustFiles(join(root, 'strands-rs'))
const allNames = new Set(allFiles.flatMap(testNames))

function check(row) {
  const result = { matched: 0, declared: 0, gap: 0, missing: [], problems: [] }
  const path = join(tsRoot, row.spec.replace(/^strands-ts\/src\//, ''))
  if (!existsSync(path)) {
    result.problems.push('spec file not found')
    return result
  }
  const found = cases(path)
  if (found.length !== row.total) result.problems.push(`spec declares ${found.length} cases, row says ${row.total}`)
  const entries = titleMap[row.spec] ?? {}
  if (row.ported === 0 && row.gaps !== '' && Object.keys(entries).length === 0) {
    result.gap = found.length
    return result
  }
  const files = filesOfRow(row.tests)
  const available = new Map()
  for (const name of (files.length > 0 ? files : allFiles).flatMap(testNames)) {
    available.set(name, (available.get(name) ?? 0) + 1)
  }
  const used = new Set()
  for (const { title, describes, line } of found) {
    const name = snakeCase(title)
    const key = [[...describes, title].join(' > '), title, '*'].find((candidate) => candidate in entries)
    const entry = key === undefined || (key === '*' && available.get(name) > 0) ? undefined : entries[key]
    if (key !== undefined) used.add(key)
    if (entry === undefined) {
      if (available.get(name) > 0) {
        available.set(name, available.get(name) - 1)
        result.matched++
      } else {
        result.missing.push({ title, name, line })
      }
    } else if (typeof entry === 'string') {
      if (allNames.has(entry.split('::').pop())) result.declared++
      else result.problems.push(`map names an unknown Rust test: ${entry}`)
    } else if (typeof entry?.gap === 'string' && entry.gap !== '') {
      result.gap++
    } else {
      result.problems.push(`map entry for "${key}" is neither a test name nor a gap`)
    }
  }
  for (const key of Object.keys(entries)) {
    if (!used.has(key)) result.problems.push(`map entry matches no case: "${key}"`)
  }
  if (result.matched + result.declared !== row.ported) {
    result.problems.push(`matched + declared is ${result.matched + result.declared}, row says ${row.ported} ported`)
  }
  return result
}

const totals = new Map()
let failed = 0
for (const row of rows()) {
  if (filters.length > 0 && !filters.some((filter) => row.spec.includes(filter))) continue
  const result = check(row)
  const bad = result.missing.length > 0 || result.problems.length > 0
  if (bad) failed++
  const counts = [result.matched, result.declared, result.gap, result.missing.length]
  console.log(`${bad ? 'FAIL' : 'ok  '} ${row.spec} ${counts.join('/')} (row ${row.ported}/${row.total})`)
  for (const problem of result.problems) console.log(`       ${problem}`)
  if (report) {
    for (const { title, name, line } of result.missing) console.log(`       :${line} ${JSON.stringify(title)} -> ${name}`)
  }
  const segments = row.spec.split('/')
  const area = segments[2] === '__tests__' ? '(root)' : segments[2]
  const total = totals.get(area) ?? { counts: [0, 0, 0, 0], specs: 0, failed: 0 }
  counts.forEach((count, at) => (total.counts[at] += count))
  total.specs++
  if (bad) total.failed++
  totals.set(area, total)
}
console.log('\narea: matched/declared/gap/missing (failing specs/specs)')
for (const [area, total] of [...totals].sort()) {
  console.log(`${area}: ${total.counts.join('/')} (${total.failed}/${total.specs})`)
}
process.exit(failed > 0 ? 1 : 0)
