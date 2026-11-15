import { pretty } from './out.js'

const dist = '../../../strands-ts/dist/src'
const sdk = await import(`${dist}/index.node.js`)
const { zodSchemaToJsonSchema } = await import(`${dist}/tools/zod-utils.js`)
const { z } = await import('zod')

interface TreeNode {
  name: string
  children: TreeNode[]
}

const TreeNodeSchema: z.ZodType<TreeNode> = z.object({
  name: z.string(),
  get children() {
    return z.array(TreeNodeSchema)
  },
})

const schemas: Record<string, z.ZodType> = {
  stringWithDescription: z.object({ city: z.string().describe('City name') }),
  optionalFields: z.object({
    required: z.string(),
    optional: z.string().optional(),
    nullable: z.string().nullable(),
    withDefault: z.number().default(3),
  }),
  enums: z.object({
    unit: z.enum(['celsius', 'fahrenheit']).describe('Temperature unit'),
    literal: z.literal('fixed'),
  }),
  nestedObjects: z.object({
    location: z.object({ lat: z.number(), lon: z.number(), label: z.string().optional() }).describe('Coordinates'),
  }),
  arrays: z.object({
    tags: z.array(z.string()),
    matrix: z.array(z.array(z.number())),
    items: z.array(z.object({ id: z.number().int(), done: z.boolean() })).min(1),
  }),
  numbers: z.object({
    float: z.number(),
    int: z.number().int(),
    bounded: z.number().int().min(0).max(100),
    positive: z.number().positive(),
  }),
  stringFormats: z.object({
    email: z.string().email(),
    url: z.string().url(),
    uuid: z.string().uuid(),
    datetime: z.string().datetime(),
    pattern: z.string().regex(/^[a-z]+$/),
    lengths: z.string().min(1).max(10),
  }),
  unions: z.object({ value: z.union([z.string(), z.number()]), maybe: z.union([z.string(), z.null()]) }),
  records: z.object({ labels: z.record(z.string(), z.string()) }),
  recursive: TreeNodeSchema,
  emptyObject: z.object({}),
  describedRoot: z.object({ q: z.string() }).describe('Search parameters'),
  strictObject: z.strictObject({ a: z.string() }),
  looseObject: z.looseObject({ a: z.string() }),
}

pretty(
  'schemas/zod_to_json_schema.json',
  Object.fromEntries(Object.entries(schemas).map(([name, schema]) => [name, z.toJSONSchema(schema)])),
  {
    api: '`z.toJSONSchema` (zod 4, root `node_modules`)',
    note: 'Raw Zod JSON Schema output, including `$schema`, for schema-normalization tests',
  }
)

pretty(
  'schemas/tool_input_schemas.json',
  Object.fromEntries(Object.entries(schemas).map(([name, schema]) => [name, zodSchemaToJsonSchema(schema)])),
  {
    api: '`zodSchemaToJsonSchema` (`tools/zod-utils.ts`)',
    note: 'The tool input schema TS sends: Zod output with only `$schema` stripped',
  }
)

const zodTool = sdk.tool({
  name: 'get_weather',
  description: 'Gets the weather',
  inputSchema: schemas.optionalFields,
  callback: () => 'ok',
})
const jsonTool = new sdk.FunctionTool({
  name: 'json_tool',
  description: 'JSON schema tool',
  inputSchema: { type: 'object', properties: { b: { type: 'string' }, a: { type: 'integer' } }, required: ['b'] },
  callback: () => 'ok',
})
const noSchemaTool = new sdk.FunctionTool({ name: 'no_schema', description: 'No input schema', callback: () => 'ok' })
pretty(
  'schemas/tool_specs.json',
  { zodTool: zodTool.toolSpec, jsonSchemaTool: jsonTool.toolSpec, noInputSchema: noSchemaTool.toolSpec },
  {
    api: '`tool({ inputSchema: zod })`, `new FunctionTool(...)` `toolSpec`',
    note: '`ToolSpec` wire shape for Zod, JSON schema and schema-less tools',
  }
)
