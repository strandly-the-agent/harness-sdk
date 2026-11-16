// Checks that every TS test case of an in-scope PARITY.md row has a Rust test named after it.
//
// Usage: node strands-rs/docs/check-test-titles.mjs [--root <repo>] [--parity <file>] [--ts <spec root>]
//          [--report] [filter...]
//   --root    repository root (default: two directories above this script)
//   --ts      TS root the PARITY.md spec paths point into (default: <root>/strands-ts/src, or
//             <root>/harness-ts/test for the harness)
//   --report  also print each missing case (its map key and the Rust name it expects), and the Rust tests of the
//             row's files that no case matched (Rust-only tests, or tests still to rename)
//   --parity  the PARITY.md to check (default: <root>/strands-rs/docs/PARITY.md). Its folder picks the suite
//             (see SUITES): `--parity harness-rs/docs/PARITY.md` checks the harness, with the map
//             `harness-rs/docs/test-title-map.json` and the Rust tests under `harness-rs/`.
//   filter    only check rows whose spec path contains one of these substrings
// Needs the `typescript` package: run it from a checkout with `node_modules`, or set `NODE_PATH`.
//
// For each row of the "In scope" table in the PARITY.md it lists the spec's cases (the calls
// count-ts-cases.mjs counts; an `it.each` case counts once, by its template text) and sorts each into:
//   matched   a Rust test function has the snake_case name of the title
//   declared  test-title-map.json names the Rust test that covers the case
//   gap       test-title-map.json gives a reason, or the row ports nothing (`0/N`) and gives one
//   missing   none of the above
// It exits 1 when a case is missing, when matched + declared differs from the row's `ported` count, or
// when the cases found differ from the row's `total`.
//
// snake_case rule, in order:
//   1. drop `it.each` placeholders: printf ones (`%s`, `%j`, `%p`, `%i`, `%d`, `%o`) and `$name` or
//      `$a.b` ones (so "maps $name on the retry" is `maps_on_the_retry`);
//   2. drop apostrophes inside words ("doesn't" is `doesnt`, "caller's" is `callers`);
//   3. split camelCase words: `_` between a lowercase letter or digit and the uppercase letter after it
//      (`toJSON` is `to_json`, `structuredOutput` is `structured_output`), and between an acronym and
//      a capitalized word of three letters or more (`APIError` is `api_error`; `PDFs` stays `pdfs`);
//   4. lowercase; every run of characters outside `a-z0-9` becomes one `_`; trim `_` from both ends;
//   5. prefix `_` when the result starts with a digit or is empty.
// A template-literal title uses its source text, so `${name}` contributes `name`. No truncation: names
// are exact. The forms that skip step 1, 2 or 3 (`maps_name_on_the_retry`, `doesn_t`,
// `structuredoutput`) are accepted too, because part of the suite was ported with them; new tests use
// the full rule.
// A Rust name may also carry the titles of the innermost enclosing `describe` blocks in front, by the
// same rule ("AfterToolsEvent" > "includes message" is `after_tools_event_includes_message`), either
// in the function name or as its enclosing `mod` names (`mod after_tools_event { fn includes_message`);
// that is how titles that repeat in a spec are told apart. The most specific name wins.
//
// Rust tests: every `fn <name>(` that follows a test attribute (`#[test]`, `#[tokio::test]`,
// `#[rstest]`, `#[test_case(..)]`, any `#[<path>::test]`) in the files the row's second column names.
// `crate::a::b` resolves to the longest existing prefix (`a/b.rs`, `a/b/mod.rs`, and everything under
// `a/b/` when the whole path resolves); a file or directory path is taken as is; a trybuild case file
// counts as a test named after its stem. A row that names no file is checked against all of
// the suite's Rust folder (`strands-rs/` or `harness-rs/`). A title that repeats in a spec (under different
// `describe` blocks) needs as many Rust tests with that name, each in its own `mod`.
//
// test-title-map.json: { "<spec path>": { "<ts title>": "<rust test name>" | { "gap": "<reason>" } } }
// The key is the title as written, or the `describe` titles and the title joined with " > " when the
// bare title repeats. The key "*" applies to every title of the spec that nothing else covers. A Rust
// test name is the function name, optionally with leading `mod::` segments; it must exist in the suite's Rust
// folder.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { basename, dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

// `require` rather than `import`, so that `NODE_PATH` can supply the package.
const ts = createRequire(import.meta.url)('typescript')

const ROOTS = new Set(['it', 'test'])
const FACTORIES = new Set(['each', 'for', 'skipIf', 'runIf'])
// Per Rust folder: the prefix of its PARITY.md spec paths, and its crates' source folders.
const SUITES = {
  'strands-rs': {
    specs: 'strands-ts/src/',
    crates: { strands: 'strands-rs/strands/src', strands_macros: 'strands-rs/strands-macros/src' },
  },
  'harness-rs': { specs: 'harness-ts/test/', crates: { strands_harness: 'harness-rs/strands-harness/src' } },
}
const TEST_ATTRIBUTE = /^\s*#\[\s*(?:\w+::)*(?:test|rstest|test_case)\b/
const FUNCTION = /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(?:r#)?(\w+)\s*[(<]/

const args = process.argv.slice(2)
const option = (name) => {
  const at = args.indexOf(name)
  return at < 0 ? undefined : args.splice(at, 2)[1]
}
const root = resolve(option('--root') ?? join(dirname(fileURLToPath(import.meta.url)), '..', '..'))
const parityPath = resolve(option('--parity') ?? join(root, 'strands-rs/docs/PARITY.md'))
const docs = dirname(parityPath)
const rustDir = relative(root, dirname(docs)).replaceAll('\\', '/')
const suite = SUITES[rustDir]
if (suite === undefined) {
  console.error(`no suite for ${parityPath}: expected <root>/{${Object.keys(SUITES).join(',')}}/docs/PARITY.md`)
  process.exit(2)
}
const tsRoot = resolve(option('--ts') ?? join(root, suite.specs))
const report = args.includes('--report')
const filters = args.filter((arg) => arg !== '--report')

function lowerSnake(text) {
  const name = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
  return /^[a-z]/.test(name) ? name : `_${name}`
}

// The accepted Rust names of a title: the canonical one first.
function snakeCase(title) {
  const steps = [
    (text) => text.replace(/%[sdifjoOp]|\$[A-Za-z_][\w.]*/g, ' '),
    (text) => text.replace(/(?<=[A-Za-z])['\u2019](?=[A-Za-z])/g, ''),
    (text) => text.replace(/([a-z0-9])([A-Z])/g, '$1_$2').replace(/([A-Z])([A-Z][a-z]{2,})/g, '$1_$2'),
  ]
  const names = []
  for (let skipped = 0; skipped < 1 << steps.length; skipped++) {
    const text = steps.reduce((text, step, at) => (skipped & (1 << at) ? text : step(text)), title)
    names.push(lowerSnake(text))
  }
  return [...new Set(names)]
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
    // The enclosing inline modules, found by indentation (the tree is rustfmt-formatted).
    let mods = []
    readFileSync(file, 'utf8')
      .split('\n')
      .forEach((line, at) => {
        const indent = line.search(/\S/)
        if (indent < 0) return
        mods = mods.filter((mod) => mod.indent < indent)
        const mod = /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*\{/.exec(line)?.[1]
        if (mod !== undefined) mods.push({ name: mod, indent })
        if (TEST_ATTRIBUTE.test(line)) attribute = true
        const name = FUNCTION.exec(line)?.[1]
        if (name !== undefined) {
          const path = mods.map((mod) => mod.name).filter((mod) => mod !== 'tests')
          const qualified = path.map((_, from) => [...path.slice(from), name].join('_'))
          if (attribute) names.push({ name, qualified: [...qualified, name], file, line: at + 1 })
          attribute = false
        }
      })
    // A trybuild case is a whole file.
    if (/[\\/]tests[\\/]ui[\\/]/.test(file)) names.push({ name: basename(file, '.rs'), qualified: [basename(file, '.rs')], file, line: 1 })
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
  const crate = suite.crates[segments[0]]
  if (crate === undefined) {
    return [join(root, token), join(root, rustDir, token)].flatMap(rustFiles)
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
  const lines = readFileSync(parityPath, 'utf8').split('\n')
  const start = lines.indexOf('## In scope')
  const end = lines.findIndex((line, at) => at > start && line.startsWith('## '))
  return lines.slice(start, end < 0 ? undefined : end).flatMap((line) => {
    const cells = line.split('|').map((cell) => cell.trim())
    const spec = /^`([^`]+)`$/.exec(cells[1] ?? '')?.[1]
    const counts = /^(\d+)\/(\d+)$/.exec(cells[3] ?? '')
    if (spec === undefined || !spec.startsWith(suite.specs) || counts === null) return []
    const gaps = cells.slice(4, -1).join('|').trim()
    return [{ spec, tests: cells[2], ported: Number(counts[1]), total: Number(counts[2]), gaps }]
  })
}

const mapPath = join(docs, 'test-title-map.json')
const titleMap = existsSync(mapPath) ? JSON.parse(readFileSync(mapPath, 'utf8')) : {}
const allFiles = rustFiles(join(root, rustDir))
const allNames = new Set(allFiles.flatMap(testNames).map((test) => test.name))

function check(row) {
  const result = { matched: 0, declared: 0, gap: 0, missing: [], problems: [], rustOnly: [] }
  const path = join(tsRoot, row.spec.slice(suite.specs.length))
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
  const tests = (files.length > 0 ? files : allFiles).flatMap(testNames)
  // Unmatched tests by name, and by name behind their enclosing `mod` names.
  const available = new Map()
  for (const test of tests) {
    for (const name of test.qualified) available.set(name, [...(available.get(name) ?? []), test])
  }
  const taken = new Set()
  const take = (name) => {
    if (name === undefined) return undefined
    const test = (available.get(name) ?? []).find((test) => !taken.has(test))
    if (test !== undefined) taken.add(test)
    return test
  }
  const free = (name) => (available.get(name) ?? []).some((test) => !taken.has(test))
  const declared = new Set()
  const used = new Set()
  const repeats = new Map()
  for (const { title } of found) repeats.set(title, (repeats.get(title) ?? 0) + 1)
  const explicit = (item) => [[...item.describes, item.title].join(' > '), item.title].find((key) => key in entries)
  // The most specific names are matched first, over the whole spec: the title behind all its `describe`
  // titles, down to the bare title.
  const depth = Math.max(0, ...found.map((item) => item.describes.length))
  for (let skip = 0; skip <= depth; skip++) {
    for (const item of found) {
      const from = item.describes.length - (depth - skip)
      if (item.test !== undefined || explicit(item) !== undefined || from < 0) continue
      const names = snakeCase([...item.describes.slice(from), item.title].join(' '))
      item.test = take(names.find(free))
    }
  }
  for (const item of found) {
    const { title, describes, line } = item
    const key = explicit(item) ?? (item.test === undefined && '*' in entries ? '*' : undefined)
    const entry = key === undefined ? undefined : entries[key]
    if (key !== undefined) used.add(key)
    if (entry === undefined) {
      if (item.test !== undefined) {
        result.matched++
      } else {
        result.missing.push({ key: repeats.get(title) > 1 ? [...describes, title].join(' > ') : title, name: snakeCase(title)[0], line })
      }
    } else if (typeof entry === 'string') {
      declared.add(entry.split('::').pop())
      if (allNames.has(entry.split('::').pop())) result.declared++
      else result.problems.push(`map names an unknown Rust test: ${entry}`)
    } else if (typeof entry?.gap === 'string' && entry.gap !== '') {
      result.gap++
    } else {
      result.problems.push(`map entry for "${key}" is neither a test name nor a gap`)
    }
  }
  if (files.length > 0) {
    result.rustOnly = tests.filter((test) => !taken.has(test) && !declared.has(test.name))
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
    for (const { key, name, line } of result.missing) console.log(`       :${line} ${JSON.stringify(key)} -> ${name}`)
    for (const { name, file, line } of result.rustOnly) console.log(`       rust-only: ${name} (${relative(root, file)}:${line})`)
  }
  const segments = row.spec.slice(suite.specs.length).split('/')
  const area = segments.length === 1 || segments[0] === '__tests__' ? '(root)' : segments[0]
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
