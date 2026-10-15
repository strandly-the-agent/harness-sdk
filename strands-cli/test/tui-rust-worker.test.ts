import { mkdtemp, rm, writeFile, chmod } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { parseArgs } from '../src/cli/arguments.js'
import { CliConfigStore } from '../src/tui/config.js'
import { resolveRustWorker, WorkerBackend } from '../src/tui/project/worker.js'

/** A stand-in for `strands-cli-worker`: speaks the worker protocol on fd 3, echoing prompts back. */
const FAKE_WORKER = `#!/usr/bin/env node
const fs = require('node:fs')
const readline = require('node:readline')
const out = fs.createWriteStream(null, { fd: 3 })
const send = (message) => out.write(JSON.stringify(message) + '\\n')
let init
const state = () => ({
  name: 'Fake Rust', reconstructable: true, description: '', model: init.overrides.model ?? 'bedrock/default',
  modelSpecifier: init.overrides.model, thinking: init.overrides.effort ?? 'auto', sessionDirectory: '/tmp/s',
  messages: [], tools: [{ name: 'shell', description: 'sh' }], skills: [], privatePaths: [],
})
readline.createInterface({ input: process.stdin }).on('line', (line) => {
  const command = JSON.parse(line)
  if (command.type === 'init') { init = command; send({ type: 'ready', state: state() }) }
  else if (command.type === 'prompt') {
    send({ type: 'event', event: { type: 'textDelta', text: 'echo: ' + command.prompt } })
    send({ type: 'result', result: { stopReason: 'endTurn', finalText: 'echo: ' + command.prompt,
      usage: { inputTokens: 1, outputTokens: 2, totalTokens: 3, cacheReadInputTokens: 0, cacheWriteInputTokens: 0 } },
      state: state(), reload: false })
  } else if (command.type === 'close') process.exit(0)
})
`

afterEach(() => {
  vi.unstubAllEnvs()
})

describe('resolveRustWorker', () => {
  it('prefers STRANDS_RUST_WORKER and rejects a missing path', async () => {
    const root = await mkdtemp(join(tmpdir(), 'strands-rust-worker-'))
    try {
      const binary = join(root, 'strands-cli-worker')
      await writeFile(binary, '')
      expect(resolveRustWorker({ STRANDS_RUST_WORKER: binary })).toBe(binary)
      expect(() => resolveRustWorker({ STRANDS_RUST_WORKER: join(root, 'nope') })).toThrow('missing file')
      expect(resolveRustWorker({ PATH: root })).toBe(binary)
      expect(() => resolveRustWorker({ PATH: join(root, 'empty'), CARGO_TARGET_DIR: join(root, 'empty') })).toThrow(
        'strands-cli-worker'
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})

describe('--runtime', () => {
  it('parses the flag and the STRANDS_RUNTIME fallback', () => {
    expect(parseArgs([]).runtime).toBeUndefined()
    expect(parseArgs(['--runtime', 'rust']).runtime).toBe('rust')
    vi.stubEnv('STRANDS_RUNTIME', 'rust')
    expect(parseArgs([]).runtime).toBe('rust')
    expect(parseArgs(['--runtime', 'typescript']).runtime).toBe('typescript')
    expect(() => parseArgs(['--runtime', 'go'])).toThrow()
  })
})

describe('WorkerBackend.openRust', () => {
  it('spawns the worker, sends the profile as overrides, and streams a turn', async () => {
    const root = await mkdtemp(join(tmpdir(), 'strands-rust-backend-'))
    try {
      const binary = join(root, 'fake-worker.cjs')
      await writeFile(binary, FAKE_WORKER)
      await chmod(binary, 0o755)
      vi.stubEnv('STRANDS_RUST_WORKER', binary)
      const backend = await WorkerBackend.openRust(CliConfigStore.memory(), {
        cwd: root,
        overrides: { model: 'bedrock/test-model', effort: 'low' },
      })
      try {
        expect(backend.id).toBe('rust')
        expect(backend.info().model).toBe('bedrock/test-model')
        expect(backend.name).toBe('Fake Rust')
        const stream = backend.stream('hi')
        const events = []
        let next = await stream.next()
        while (!next.done) {
          events.push(next.value)
          next = await stream.next()
        }
        expect(events).toEqual([{ type: 'textDelta', text: 'echo: hi' }])
        expect(next.value.stopReason).toBe('endTurn')
        expect(next.value.usage?.totalTokens).toBe(3)
      } finally {
        await backend.dispose()
      }
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})
