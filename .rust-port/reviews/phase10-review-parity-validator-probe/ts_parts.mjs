import { Tracer } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/telemetry/tracer.js'
import { jsonReplacer } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/telemetry/json.js'
import { GuardContentBlock, TextBlock, UnknownBlock } from '/mnt/workspace/harness-sdk/strands-ts/dist/src/types/messages.js'
const sys = [
  new TextBlock('be nice'),
  new GuardContentBlock({ text: { qualifiers: ['query'], text: 'guarded' } }),
  new GuardContentBlock({ image: { format: 'png', source: { bytes: new TextEncoder().encode('hello world') } } }),
]
console.log(JSON.stringify(Tracer._mapSystemPromptToOtelParts(sys), jsonReplacer))
const content = [new TextBlock('first'), new UnknownBlock({ futureThing: { a: 1 } })]
console.log(JSON.stringify(Tracer._mapContentBlocksToOtelParts(content), jsonReplacer))
