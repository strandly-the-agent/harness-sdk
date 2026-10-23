import type { JSONSchema } from '../types/json.js'
import type { ToolSpec } from './types.js'

const COMPOSITE_SCHEMA_KEYS = ['$ref', 'allOf', 'anyOf', 'oneOf', 'not', 'if', 'then', 'else'] as const

/**
 * Returns a copy of `toolSpec` with an optional `name` property added to its input schema,
 * or `undefined` when the schema cannot safely take one (composite or non-object schema,
 * or the property already exists).
 *
 * @internal
 */
export function addInputProperty(toolSpec: ToolSpec, name: string, property: JSONSchema): ToolSpec | undefined {
  const schema = toolSpec.inputSchema ?? {}
  if (
    COMPOSITE_SCHEMA_KEYS.some((key) => schema[key] !== undefined) ||
    (schema.type !== undefined && schema.type !== 'object') ||
    (schema.properties !== undefined && (typeof schema.properties !== 'object' || Array.isArray(schema.properties))) ||
    schema.properties?.[name] !== undefined ||
    schema.required?.includes(name)
  ) {
    return
  }
  return {
    ...toolSpec,
    inputSchema: {
      ...schema,
      type: 'object',
      properties: { ...schema.properties, [name]: property },
    },
  }
}
