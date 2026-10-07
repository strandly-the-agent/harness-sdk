// Repro for "write tool fails on Windows" in the Strands harness.
// Exercises the three code paths involved, with no model in the loop:
//   1. harness `write` tool path validation (harness-ts/src/tools/file-tools.ts validatePath)
//   2. CLI sandbox: PosixShellSandbox.writeFile -> `sh -c "mkdir -p ... && base64 -d ..."`
//      (strands-cli/src/tui/workspace/sandbox.ts does not override writeFile)
//   3. library default sandbox: NotASandboxLocalEnvironment.writeFile -> fs/promises
import { spawn } from 'node:child_process'
import { mkdtempSync, existsSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { PosixShellSandbox, Agent } from '@strands-agents/sdk'
import { write } from '@strands-agents/harness'

const label = process.argv[2] ?? process.platform
const dir = mkdtempSync(join(tmpdir(), 'strands-win-repro-'))
console.log(`== ${label} | node ${process.version} | ${process.platform} | cwd ${process.cwd()} | tmp ${dir}`)

// Same shape as strands-cli WorkspaceSandbox: PosixShellSandbox + spawn('sh', ['-c', cmd]).
class WorkspaceLikeSandbox extends PosixShellSandbox {
  constructor(cwd) { super(); this.cwd = resolve(cwd) }
  async *executeStreaming(command) {
    const child = spawn('sh', ['-c', command], { cwd: this.cwd, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = '', stderr = ''
    child.stdout.on('data', (d) => (stdout += d))
    child.stderr.on('data', (d) => (stderr += d))
    const exitCode = await new Promise((res, rej) => {
      child.on('error', rej)
      child.on('close', (code) => res(code ?? 1))
    })
    yield { type: 'executionResult', exitCode, stdout, stderr, outputFiles: [] }
  }
}

async function attempt(name, fn) {
  try {
    const r = await fn()
    console.log(`[OK]   ${name}${r ? ` -> ${r}` : ''}`)
  } catch (e) {
    console.log(`[FAIL] ${name} -> ${e?.constructor?.name}: ${e?.message?.split('\n')[0]}`)
  }
}

const check = (p) => (existsSync(p) ? `file exists at ${p}` : `NO FILE at ${p}`)

// --- 1. harness `write` tool, default (library) sandbox: validation only cares about leading '/'
const agent = new Agent({ model: undefined, tools: [] })
const ctx = { agent }
const winPath = join(dir, 'novel.json')               // C:\Users\...\novel.json on Windows
const slashPath = winPath.replace(/\\/g, '/')          // C:/Users/... (still rejected: no leading '/')
const rootless = '/' + slashPath.replace(/^[A-Za-z]:\//, '') // /Users/.../novel.json
await attempt(`harness write tool, path=${winPath}`, async () => { await write.invoke({ path: winPath, content: '{}' }, ctx); return check(winPath) })
await attempt(`harness write tool, path=${slashPath}`, async () => { await write.invoke({ path: slashPath, content: '{}' }, ctx); return check(winPath) })
await attempt(`harness write tool, path=${rootless}`, async () => { await write.invoke({ path: rootless, content: '{}' }, ctx); return check(rootless) + ' / ' + check(resolve(rootless)) })

// --- 2. CLI-style sandbox (PosixShellSandbox.writeFile via `sh`)
const ws = new WorkspaceLikeSandbox(dir)
await attempt(`WorkspaceSandbox.writeText ${winPath}`, async () => { await ws.writeText(winPath, 'hi'); return check(winPath) })
await attempt(`WorkspaceSandbox.writeText ${rootless}`, async () => { await ws.writeText(rootless, 'hi'); return check(rootless) + ' / ' + check(resolve(rootless)) })
await attempt(`WorkspaceSandbox.writeText relative chapter.md`, async () => { await ws.writeText('chapter.md', 'hi'); return check(join(dir, 'chapter.md')) })
await attempt(`WorkspaceSandbox.execute pwd`, async () => (await ws.execute('pwd')).stdout.trim())
await attempt(`WorkspaceSandbox.execute uname -s`, async () => (await ws.execute('uname -s')).stdout.trim())

// --- 3. library default sandbox (fs/promises) for contrast
await attempt(`default sandbox writeText ${winPath}`, async () => { await agent.sandbox.writeText(winPath, 'hi'); return check(winPath) + ' content=' + readFileSync(winPath, 'utf8') })
