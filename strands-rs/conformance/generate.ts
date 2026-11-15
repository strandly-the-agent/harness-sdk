// Regenerate with `npx tsx strands-rs/conformance/generate.ts` from the repo root (see README.md).
import './src/determinism.js'

const { resetFixturesDir, writeIndex } = await import('./src/out.js')

resetFixturesDir()
await import('./src/types.js')
await import('./src/events.js')
await import('./src/persistence.js')
await import('./src/schemas.js')
await import('./src/native-output.js')
await import('./src/telemetry.js')
await import('./src/bedrock.js')
writeIndex()
