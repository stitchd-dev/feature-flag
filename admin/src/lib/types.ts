// Shared API response types matching the gateway's AdminFlagJson shape.

export interface VariantJson {
  key: string
  value: unknown
}

export interface RuleJson {
  /** ConditionExpr serde JSON — see ruleTypes.ts for the full type */
  condition: unknown
  /** Gateway output JSON — `{variant_key: "..."}` or `{allocation: [...]}` */
  output: unknown
}

export interface AdminFlagResponse {
  flag_id: string
  key: string
  name: string
  description: string
  flag_type: 'bool' | 'string' | 'int' | 'double' | 'json'
  enabled: boolean
  status: 'enabled' | 'disabled'
  version: number
  variants: VariantJson[]
  rules: RuleJson[]
  default_variant_key: string | null
  created_at: string | null
  updated_at: string | null
}
