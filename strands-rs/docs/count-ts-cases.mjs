// Counts the test cases declared in TS spec files, for the `total` column of PARITY.md.
//
// Usage (from the repo root): node strands-rs/docs/count-ts-cases.mjs <file-or-dir>...
// Directories are walked for `*.test.ts` and `*.test.node.ts`. Prints `<count>\t<path>` per file.
//
// A case is one call expression rooted at `it` or `test`: `it(..)`, `it.only(..)`, `it.skip(..)`,
// `it.todo(..)`, `it.concurrent(..)`, and the curried forms `it.each(..)(..)`, `it.each<T>(..)(..)`,
// `it.for(..)(..)`, `it.skipIf(..)(..)`, `it.runIf(..)(..)`. A parameterized case counts once.

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import ts from 'typescript'

const ROOTS = new Set(['it', 'test'])
const FACTORIES = new Set(['each', 'for', 'skipIf', 'runIf'])

// Peels modifiers and factory calls off a callee; returns the root identifier name, if any.
function rootName(node) {
  while (true) {
    if (ts.isIdentifier(node)) return node.text
    if (ts.isPropertyAccessExpression(node)) node = node.expression
    else if (ts.isCallExpression(node)) node = node.expression
    else return undefined
  }
}

function isCase(call) {
  const callee = call.expression
  // `it.each(table)` only builds the case; the call on its result is the case.
  if (ts.isPropertyAccessExpression(callee) && FACTORIES.has(callee.name.text)) return false
  return ROOTS.has(rootName(callee))
}

function count(path) {
  const source = ts.createSourceFile(path, readFileSync(path, 'utf8'), ts.ScriptTarget.Latest, true)
  let n = 0
  const visit = (node) => {
    if (ts.isCallExpression(node) && isCase(node)) n++
    ts.forEachChild(node, visit)
  }
  visit(source)
  return n
}

function specs(path) {
  if (!statSync(path).isDirectory()) return [path]
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const child = join(path, entry.name)
    if (entry.isDirectory()) return entry.name === 'node_modules' ? [] : specs(child)
    return /\.test(\.node)?\.ts$/.test(entry.name) ? [child] : []
  })
}

for (const path of process.argv.slice(2).flatMap(specs).sort()) {
  console.log(`${count(path)}\t${path}`)
}
