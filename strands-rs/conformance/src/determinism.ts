// Must be evaluated before strands-ts (or uuid) is imported: everything that stamps time or
// randomness into a fixture reads these globals.

export const FIXED_TIME_MS = Date.UTC(2026, 0, 1, 0, 0, 0, 0)

const RealDate = Date

class FixedDate extends RealDate {
  constructor(...args: unknown[]) {
    if (args.length === 0) {
      super(FIXED_TIME_MS)
    } else {
      super(...(args as [string | number | Date]))
    }
  }

  static now(): number {
    return FIXED_TIME_MS
  }
}

globalThis.Date = FixedDate as DateConstructor

Object.defineProperty(globalThis.performance, 'now', { value: () => 0, configurable: true, writable: true })

let uuidCounter = 0
const randomUUID = (): `${string}-${string}-${string}-${string}-${string}` => {
  uuidCounter += 1
  return `00000000-0000-4000-8000-${uuidCounter.toString(16).padStart(12, '0')}`
}

let xorshiftState = 0x2545f491
function nextUint32(): number {
  let x = xorshiftState
  x ^= x << 13
  x ^= x >>> 17
  x ^= x << 5
  xorshiftState = x >>> 0
  return xorshiftState
}

const getRandomValues = <T extends ArrayBufferView | null>(array: T): T => {
  if (array === null) return array
  const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength)
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = nextUint32() & 0xff
  }
  return array
}

const cryptoObject = globalThis.crypto
Object.defineProperty(cryptoObject, 'randomUUID', { value: randomUUID, configurable: true, writable: true })
Object.defineProperty(cryptoObject, 'getRandomValues', { value: getRandomValues, configurable: true, writable: true })

Math.random = () => nextUint32() / 0x100000000

// Telemetry reads these at construction time; the fixtures pin the defaults.
for (const name of Object.keys(process.env)) {
  if (name.startsWith('OTEL_')) delete process.env[name]
}
