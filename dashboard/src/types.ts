export interface ProcessStatus {
  running: boolean
  pid: number | null
  started_at: string | null
  uptime_secs: number | null
}

export interface GatewaySettings {
  max_history?: number | null
  max_iterations?: number | null
  system_prompt?: string | null
  max_concurrent_sessions?: number | null
}

export interface ChannelCredentials {
  type: string
  [key: string]: string | number
}

export interface ProfileConfig {
  provider?: string | null
  model?: string | null
  api_key_env?: string | null
  channels: ChannelCredentials[]
  gateway: GatewaySettings
  env_vars: Record<string, string>
}

export interface UserProfile {
  id: string
  name: string
  enabled: boolean
  data_dir: string | null
  config: ProfileConfig
  created_at: string
  updated_at: string
}

export interface ProfileResponse {
  id: string
  name: string
  enabled: boolean
  data_dir: string | null
  config: ProfileConfig
  created_at: string
  updated_at: string
  status: ProcessStatus
}

export interface OverviewResponse {
  total_profiles: number
  running: number
  stopped: number
  profiles: ProfileResponse[]
}

export interface ActionResponse {
  ok: boolean
  message?: string
}

export interface BulkActionResponse {
  ok: boolean
  count: number
}

export type ChannelType = 'telegram' | 'discord' | 'slack' | 'whatsapp' | 'feishu' | 'email'

export const CHANNEL_TYPES: ChannelType[] = ['telegram', 'discord', 'slack', 'whatsapp', 'feishu', 'email']

export const CHANNEL_COLORS: Record<ChannelType, string> = {
  telegram: 'bg-blue-500',
  discord: 'bg-indigo-500',
  slack: 'bg-purple-500',
  whatsapp: 'bg-green-500',
  feishu: 'bg-cyan-500',
  email: 'bg-orange-500',
}

export const CHANNEL_LABELS: Record<ChannelType, string> = {
  telegram: 'TG',
  discord: 'DC',
  slack: 'SL',
  whatsapp: 'WA',
  feishu: 'FS',
  email: 'EM',
}

export const PROVIDERS = [
  'anthropic', 'openai', 'gemini', 'openrouter', 'deepseek',
  'groq', 'moonshot', 'dashscope', 'minimax', 'zhipu', 'ollama', 'vllm',
] as const
