import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { compact, pretty } from './out.js'
import { ScriptedModel, turns } from './scripted-model.js'

const dist = '../../../strands-ts/dist/src'
const sdk = await import(`${dist}/index.node.js`)
const storageMod = await import(`${dist}/storage/index.js`)
const experimental = await import(`${dist}/experimental/index.js`)
const multiagent = await import(`${dist}/multiagent/index.js`)
const { SnapshotStorageAdapter } = await import(`${dist}/session/snapshot-storage-adapter.js`)
const { Stash } = await import(`${dist}/context-manager/stash.js`)
const { InterruptState, Interrupt } = await import(`${dist}/interrupt.js`)
const { InMemoryStorage, LocalFileStorage } = storageMod

function errorText(fn: () => unknown): string {
  try {
    fn()
  } catch (error) {
    return (error as Error).message
  }
  throw new Error('expected an error')
}

/** Writes every key under `prefix` byte-for-byte to `persisted/<dir>/<key>`. */
async function dumpStorage(
  storage: any,
  dir: string,
  meta: { api: string; phase: 4 | 6; note: string }
): Promise<string[]> {
  const keys: string[] = (await storage.list('')).sort()
  for (const key of keys) {
    compact(`persisted/${dir}/${key}`, await storage.read(key), meta)
  }
  return keys
}

// Snapshot presets and field selection on a populated agent.
{
  const model = new ScriptedModel([turns.text('Noted.')])
  const agent = new sdk.Agent({
    model,
    printer: false,
    systemPrompt: [{ text: 'You are helpful.' }, { cachePoint: { cacheType: 'default' } }],
    appState: { counter: 3, nested: { list: [1, 'two', null] } },
    modelState: { previousResponseId: 'resp_1' },
  })
  await agent.invoke('Remember this')
  pretty(
    'snapshots/take_snapshot.json',
    {
      sessionPreset: agent.takeSnapshot({ preset: 'session' }),
      includeMessagesOnly: agent.takeSnapshot({ include: ['messages'] }),
      presetExcludingMessages: agent.takeSnapshot({ preset: 'session', exclude: ['messages', 'interrupts'] }),
      withAppData: agent.takeSnapshot({ include: ['state'], appData: { label: 'checkpoint-1', version: 2 } }),
    },
    {
      api: '`Agent.takeSnapshot()` with preset, include, exclude, appData',
      phase: 6,
      note: 'Agent-scope snapshot shape, field order, `createdAt` in `toISOString()` form, null `interrupts`',
    }
  )
  const bare = new sdk.Agent({ model: new ScriptedModel([]), printer: false })
  pretty('snapshots/take_snapshot_empty_agent.json', bare.takeSnapshot({ preset: 'session' }), {
    api: '`Agent.takeSnapshot({ preset: "session" })`',
    phase: 6,
    note: 'Fresh agent: empty messages, null `systemPrompt` and `interrupts`',
  })
  pretty(
    'snapshots/take_snapshot_errors.json',
    {
      invalidPreset: errorText(() => agent.takeSnapshot({ preset: 'everything' })),
      invalidField: errorText(() => agent.takeSnapshot({ include: ['history'] })),
      noFields: errorText(() => agent.takeSnapshot({})),
      wrongScope: errorText(() =>
        agent.loadSnapshot({ ...agent.takeSnapshot({ preset: 'session' }), scope: 'multiAgent' })
      ),
      wrongVersion: errorText(() =>
        agent.loadSnapshot({ ...agent.takeSnapshot({ preset: 'session' }), schemaVersion: '2.0' })
      ),
    },
    { api: '`takeSnapshot`, `loadSnapshot` (`agent/snapshot.ts`)', phase: 6, note: 'Exact error texts' }
  )
}

// Session manager writing latest and immutable history, then the manifest written explicitly.
{
  const storage = new InMemoryStorage()
  const sessionManager = new sdk.SessionManager({
    sessionId: 'conformance-session',
    storage,
    snapshotTrigger: () => true,
  })
  const model = new ScriptedModel([turns.text('First answer.'), turns.text('Second answer.')])
  const agent = new sdk.Agent({ id: 'main-agent', model, printer: false, sessionManager })
  await agent.invoke('First question')
  await agent.invoke('Second question')

  const adapter = new SnapshotStorageAdapter(storage.namespace('session'))
  const location = { sessionId: 'conformance-session', scope: 'agent', scopeId: 'main-agent' }
  const defaultManifest = await adapter.loadManifest({ location })
  await adapter.saveManifest({ location, manifest: { schemaVersion: '1.0', updatedAt: new Date().toISOString() } })
  const keys = await dumpStorage(storage, 'session_agent', {
    api: '`SessionManager` (`snapshotTrigger`) and `SnapshotStorageAdapter.saveManifest` over `InMemoryStorage`',
    phase: 6,
    note: 'Persisted agent snapshot, immutable history (uuid v7 ids) and manifest bytes',
  })
  pretty(
    'persisted/session_agent.keys.json',
    {
      keys,
      historyIds: await adapter.listSnapshotIds({ location }),
      loadManifestWhenMissing: defaultManifest,
    },
    {
      api: '`Storage.list`, `SnapshotStorageAdapter.listSnapshotIds`, `loadManifest`',
      phase: 6,
      note: 'Key layout `session/<sessionId>/scopes/agent/<scopeId>/snapshots/...` and the default manifest',
    }
  )
}

// Multi-agent scope: a two-node graph persisted through the session manager.
{
  const storage = new InMemoryStorage()
  const sessionManager = new sdk.SessionManager({ sessionId: 'graph-session', storage })
  const graph = new multiagent.Graph({
    id: 'conformance-graph',
    nodes: [
      new sdk.Agent({ id: 'writer', model: new ScriptedModel([turns.text('Draft.')]), printer: false }),
      new sdk.Agent({ id: 'editor', model: new ScriptedModel([turns.text('Edited.')]), printer: false }),
    ],
    edges: [['writer', 'editor']],
    maxSteps: 10,
    sessionManager,
  })
  const result = await graph.invoke('Write something')
  await dumpStorage(storage, 'session_graph', {
    api: '`Graph` with `SessionManager` over `InMemoryStorage`',
    phase: 6,
    note: 'Multi-agent scope snapshot (`scopes/multiAgent/<graphId>`)',
  })
  pretty('snapshots/multi_agent_result.json', result, {
    api: '`MultiAgentResult.toJSON()`',
    phase: 6,
    note: 'Graph result shape with node results',
  })
}

// Context-manager stash records on ephemeral and durable storage.
{
  const baseDir = mkdtempSync(join(tmpdir(), 'strands-conformance-'))
  try {
    for (const [label, storage] of [
      ['in_memory', new InMemoryStorage()],
      ['local_file', new LocalFileStorage(baseDir)],
    ] as const) {
      const sessionManager = new sdk.SessionManager({ sessionId: 'stash-session', storage })
      const agent = new sdk.Agent({
        id: 'stash-agent',
        model: new ScriptedModel([turns.text('Stashed reply.')]),
        printer: false,
        storage,
        sessionManager,
        contextManager: new experimental.ContextManager({ strategies: [] }),
      })
      await agent.invoke('Stash this message')
      await dumpStorage(storage, `stash_${label}`, {
        api: `\`ContextManager\` stash and \`SessionManager\` over \`${storage.constructor.name}\``,
        phase: 6,
        note: 'Stash records (`context/<sessionId>/scopes/agent/<agentId>/<trackingId>_<index>`) and `data.stash` in the snapshot',
      })
    }

    const records: Record<string, unknown> = {}
    for (const storage of [new InMemoryStorage(), new LocalFileStorage(baseDir)]) {
      const stash = new Stash(storage, 'direct-session', 'direct-agent')
      const message = new sdk.Message({
        role: 'user',
        trackingId: 'tracking-stash',
        content: [
          new sdk.TextBlock('text block'),
          new sdk.ImageBlock({ format: 'png', source: { bytes: new Uint8Array([1, 2]) } }),
          new sdk.ToolResultBlock({
            toolUseId: 'tooluse_stash',
            status: 'success',
            content: [new sdk.TextBlock('result text'), new sdk.JsonBlock({ json: { k: 1 } })],
          }),
          new sdk.ToolUseBlock({ name: 'skipped', toolUseId: 'tooluse_skip', input: {} }),
        ],
      })
      await stash.storeMessage(message)
      records[stash.storageTypeName] = {
        storageTypeName: stash.storageTypeName,
        keys: (await stash.list()).sort(),
        snapshot: await stash.takeSnapshot(),
      }
    }
    pretty('stash/stash_records.json', records, {
      api: '`Stash.storeMessage`, `Stash.list`, `Stash.takeSnapshot` (`context-manager/stash.ts`)',
      phase: 6,
      note: '`storageTypeName` from `constructor.name`, record keys and decoded record bodies per block kind',
    })
  } finally {
    rmSync(baseDir, { recursive: true, force: true })
  }
}

// Interrupt state built directly: context, a null response and resume().
{
  const state = new InterruptState()
  state.activate()
  state.registerInterrupt(
    new Interrupt({ id: 'tool:tooluse_1:confirm', name: 'confirm', reason: 'Proceed?', source: 'tool' })
  )
  state.registerInterrupt(
    new Interrupt({
      id: 'hook:beforeToolCall:tooluse_2:approve',
      name: 'approve',
      reason: { risk: 'high' },
      source: 'hook',
    })
  )
  state.context = { interruptedTurns: { tooluse_1: { turn: 1 } } }
  const beforeResume = state.toJSON()
  state.resume([
    new sdk.InterruptResponseContent({ interruptId: 'tool:tooluse_1:confirm', response: null }),
    new sdk.InterruptResponseContent({
      interruptId: 'hook:beforeToolCall:tooluse_2:approve',
      response: { approved: true },
    }),
  ])
  const afterResume = JSON.parse(JSON.stringify(state))
  const restored = InterruptState.fromJSON(afterResume)
  pretty(
    'interrupts/interrupt_state.json',
    {
      beforeResume: JSON.parse(JSON.stringify(beforeResume)),
      afterResume,
      restoredUnanswered: restored.getUnansweredInterrupts().map((i: any) => i.id),
      legacyWithoutSource: new Interrupt({ id: 'x', name: 'legacy' }).toJSON(),
      resumeUnknownId: errorText(() =>
        state.resume([new sdk.InterruptResponseContent({ interruptId: 'missing', response: 1 })])
      ),
    },
    {
      api: '`InterruptState.toJSON()`, `resume()`, `fromJSON()` (`src/interrupt.ts`)',
      phase: 4,
      note: '`interrupt_response_null`: a `null` response counts as answered; `context` kept; legacy `source` default; unknown id error text',
    }
  )
}
