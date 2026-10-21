import { describe, it, expect } from 'vitest'
import { createRetrievalTool, RETRIEVAL_TOOL_NAME } from '../retrieval-tool.js'
import { Stash } from '../stash.js'
import { InMemoryStorage } from '../../storage/in-memory-storage.js'
import { ImageBlock } from '../../types/media.js'

function invoke(retrievalTool: ReturnType<typeof createRetrievalTool>, input: unknown): Promise<unknown> {
  return (retrievalTool as unknown as { invoke(input: unknown): Promise<unknown> }).invoke(input)
}

function encodeJSON(value: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(value))
}

describe('retrieval tool', () => {
  function makeStashWithTextBlock(text: string): { stash: Stash; refPromise: Promise<string> } {
    const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
    const refPromise = stash.store('tool-1', 0, encodeJSON({ text }))
    return { stash, refPromise }
  }

  it('has the correct tool name', () => {
    const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
    const retrievalTool = createRetrievalTool(stash)
    expect(retrievalTool.name).toBe(RETRIEVAL_TOOL_NAME)
  })

  it('retrieves full text content', async () => {
    const { stash, refPromise } = makeStashWithTextBlock('hello world\nline two')
    const ref = await refPromise
    const retrievalTool = createRetrievalTool(stash)

    const result = await invoke(retrievalTool, { reference: ref })
    expect(result).toEqual({ text: 'hello world\nline two' })
  })

  it('searches with pattern', async () => {
    const text = Array.from(
      { length: 20 },
      (_, index) => `line ${index + 1}: ${index % 3 === 0 ? 'ERROR' : 'ok'}`
    ).join('\n')
    const { stash, refPromise } = makeStashWithTextBlock(text)
    const ref = await refPromise
    const retrievalTool = createRetrievalTool(stash)

    const result = (await invoke(retrievalTool, { reference: ref, pattern: 'ERROR' })) as string
    expect(result).toContain('ERROR')
    expect(result).toContain('match')
  })

  it('returns line range', async () => {
    const text = Array.from({ length: 50 }, (_, index) => `line ${index + 1}`).join('\n')
    const { stash, refPromise } = makeStashWithTextBlock(text)
    const ref = await refPromise
    const retrievalTool = createRetrievalTool(stash)

    const result = (await invoke(retrievalTool, { reference: ref, line_range: { start: 5, end: 10 } })) as string
    expect(result).toContain('line 5')
    expect(result).toContain('line 10')
    expect(result).not.toContain('line 11')
  })

  it('returns error for unknown reference', async () => {
    const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
    const retrievalTool = createRetrievalTool(stash)

    const result = (await invoke(retrievalTool, { reference: 'nonexistent' })) as string
    expect(result).toContain('Error: reference not found')
  })

  it('restores image content as an ImageBlock instead of raw JSON', async () => {
    const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
    const imageData = { image: { format: 'png', source: { bytes: 'iVBORw0KGgo=' } } }
    const ref = await stash.store('tool-img', 0, encodeJSON(imageData))
    const retrievalTool = createRetrievalTool(stash)

    const result = await invoke(retrievalTool, { reference: ref })
    expect(result).toBeInstanceOf(ImageBlock)
    expect((result as ImageBlock).format).toBe('png')
  })

  it('retrieves JSON content as parsed object', async () => {
    const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
    const json = { key: 'value', nested: [1, 2, 3] }
    const ref = await stash.store('tool-json', 0, encodeJSON({ json }))
    const retrievalTool = createRetrievalTool(stash)

    const result = await invoke(retrievalTool, { reference: ref })
    expect(result).toEqual({ json })
  })

  it('returns error when searching non-text content', async () => {
    const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
    const imageData = { image: { format: 'png', source: { bytes: 'iVBORw0KGgo=' } } }
    const ref = await stash.store('tool-img', 0, encodeJSON(imageData))
    const retrievalTool = createRetrievalTool(stash)

    const result = (await invoke(retrievalTool, { reference: ref, pattern: 'test' })) as string
    expect(result).toContain('cannot search non-text content')
  })

  describe('shell-style results (#4741)', () => {
    // An object return becomes a JsonBlock, stashed as `{ json: {...} }`.
    async function storeShell(
      output: string,
      error = '',
      exit_code = 0
    ): Promise<{ retrievalTool: ReturnType<typeof createRetrievalTool>; ref: string }> {
      const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
      const ref = await stash.store('shell-1', 0, encodeJSON({ json: { output, error, exit_code } }))
      return { retrievalTool: createRetrievalTool(stash), ref }
    }

    it('pattern matches the stdout line by number', async () => {
      const output = Array.from({ length: 2000 }, (_, i) =>
        i + 1 === 861 ? 'Call me Ishmael.' : `line ${i + 1}`
      ).join('\n')
      const { retrievalTool, ref } = await storeShell(output)

      const result = (await invoke(retrievalTool, { reference: ref, pattern: 'Ishmael' })) as string
      expect(result).toContain('[1 match')
      expect(result).toContain('> 861| Call me Ishmael.')
      expect(result).not.toContain('\\n')
    })

    it('line_range indexes stdout lines', async () => {
      const output = Array.from({ length: 2000 }, (_, i) => `line ${i + 1}`).join('\n') + '\n'
      const { retrievalTool, ref } = await storeShell(output)

      const result = (await invoke(retrievalTool, { reference: ref, line_range: { start: 861, end: 865 } })) as string
      expect(result).toContain('861| line 861')
      expect(result).toContain('865| line 865')
      expect(result).not.toContain('line 866')
    })

    it('full retrieval returns stdout, then stderr and exit code', async () => {
      const { retrievalTool, ref } = await storeShell('out 1\nout 2\n', 'boom\nmore', 2)

      const result = await invoke(retrievalTool, { reference: ref })
      expect(result).toBe('out 1\nout 2\n\n[stderr]\nboom\nmore\n[exit_code: 2]')
    })

    it('stderr is searchable when stdout is empty', async () => {
      const error = Array.from({ length: 50 }, (_, i) => `err ${i + 1}`).join('\n')
      const { retrievalTool, ref } = await storeShell('', error, 1)

      const result = (await invoke(retrievalTool, { reference: ref, pattern: 'err 42' })) as string
      expect(result).toContain('[1 match')
      // line 1 is the empty stdout, line 2 the [stderr] header
      expect(result).toContain('> 44| err 42\n')
    })

    it('omits the stderr section when empty', async () => {
      const { retrievalTool, ref } = await storeShell('ok')
      expect(await invoke(retrievalTool, { reference: ref })).toBe('ok\n[exit_code: 0]')
    })

    it('also renders the Python text-block form', async () => {
      const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
      const text = JSON.stringify({ output: 'a\nb', error: '', exit_code: 0 })
      const ref = await stash.store('shell-1', 0, encodeJSON({ text }))

      expect(await invoke(createRetrievalTool(stash), { reference: ref })).toBe('a\nb\n[exit_code: 0]')
    })

    it.each([
      { output: 'a', error: '', exit_code: 0, host: 'db-1' },
      { output: 'a', error: '' },
      { output: 'a', error: '', exit_code: '0' },
      { output: ['a'], error: '', exit_code: 0 },
    ])('keeps generic JSON retrieval for other shapes: %j', async (json) => {
      const stash = new Stash(new InMemoryStorage(), 'test-session', 'test-agent')
      const ref = await stash.store('tool-json', 0, encodeJSON({ json }))

      expect(await invoke(createRetrievalTool(stash), { reference: ref })).toEqual({ json })
    })
  })
})
