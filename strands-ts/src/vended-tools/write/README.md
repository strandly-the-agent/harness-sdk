# Write Tool

A sandbox-routed tool that creates or overwrites a text file with the full provided content.

The tool writes through a [Sandbox](../../sandbox/base.ts), so behaviour is identical on the host and in Docker/SSH containers. It is part of the focused file-tool set (`read`, `edit`, `write`) that supersedes the deprecated `fileEditor`.

## Usage

```typescript
import { Agent } from '@strands-agents/sdk'
import { write } from '@strands-agents/sdk/vended-tools/write'

const agent = new Agent({ tools: [write] })
await agent.invoke("Create /tmp/hello.txt containing 'Hello, Strands!'")
```

With a sandbox bound at creation time:

```typescript
import { DockerSandbox } from '@strands-agents/sdk/sandbox/docker'
import { makeWrite } from '@strands-agents/sdk/vended-tools/write'

const sandbox = new DockerSandbox({ container: 'my-container' })
const writeTool = makeWrite(sandbox)
```

Without a bound sandbox, the tool reads `context.agent.sandbox` at call time.

## API

### `write`

The default tool, produced by `makeWrite()`.

### `makeWrite(options?)` / `makeWrite(sandbox, options?)`

| Option        | Type     | Default    | Description                     |
| ------------- | -------- | ---------- | ------------------------------- |
| `name`        | `string` | `write`    | Tool name.                      |
| `description` | `string` | (built-in) | Description shown to the model. |

### Input

```typescript
{
  path: string // Absolute path to the file
  content: string // The full file content to write
}
```

### Return Value

A confirmation string of the form `Wrote <N> lines to <path>.`, where `N` is the number of `\n`-separated entries in the content. Content ending in a newline reports one more entry than text lines; empty content reports `0 lines`.

### Error Handling

The tool throws when `path` is not absolute (POSIX `/`, a Windows drive letter, or a UNC share) or contains a `..` segment. Validation is lexical; filesystem semantics are the sandbox's responsibility.

## Security

This tool writes files at arbitrary absolute paths with the full permissions of the process. Only use with trusted input and consider running in a sandboxed environment for production.
