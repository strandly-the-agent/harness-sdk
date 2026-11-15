import { pretty } from './out.js'

const dist = '../../../strands-ts/dist/src'
const { zodSchemaToJsonSchema } = await import(`${dist}/tools/zod-utils.js`)
const { transformJSONSchema } = await import('@anthropic-ai/sdk/lib/transform-json-schema')
const { z } = await import('zod')

const Person = z.object({ name: z.string(), age: z.number().int() })
const Roster = z
  .object({
    members: z.array(Person).min(2),
    lead: Person.nullable().optional(),
    code: z
      .string()
      .regex(/^[A-Z]+$/)
      .describe('Team code'),
    founded: z.iso.date(),
    score: z.number().min(0),
  })
  .describe('A team of people')
const Team = Roster.extend({ color: z.enum(['red', 'blue']), kind: z.literal(['a', 'b']) })
const WithRecord = z.object({ scores: z.record(z.string(), z.number()) })
const WithNestedRecord = z.object({ team: Roster, extra: z.array(WithRecord) })
const WithTuple = z.object({ point: z.tuple([z.number(), z.number()]) })
const WithAny = z.object({ value: z.any() })
const WithMixedLiteral = z.object({ value: z.literal(['a', 1]) })
const WithCatchall = z.object({ name: z.string() }).catchall(z.string())
const Enum = z.object({ color: z.enum(['red', 'blue']) })

const schemas = Object.fromEntries(
  Object.entries({
    Person,
    Roster,
    Team,
    WithRecord,
    WithNestedRecord,
    WithTuple,
    WithAny,
    WithMixedLiteral,
    WithCatchall,
    Enum,
  }).map(([name, schema]) => [name, zodSchemaToJsonSchema(schema)])
)
const transformed = Object.fromEntries(
  ['Person', 'Roster', 'WithNestedRecord'].map((name) => [name, transformJSONSchema(schemas[name])])
)

pretty(
  'schemas/native_output_schemas.json',
  { schemas, transformed },
  {
    api: '`zodSchemaToJsonSchema` (`tools/zod-utils.ts`), `transformJSONSchema` (`@anthropic-ai/sdk/lib/transform-json-schema`)',
    note: 'The Zod schemas of `native-output.test.ts` as TS sends them, and the Anthropic SDK conversion of three of them',
  }
)
