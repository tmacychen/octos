import { useState } from 'react'
import type { ProfileConfig, ChannelCredentials, ChannelType } from '../types'
import { PROVIDERS, CHANNEL_TYPES } from '../types'

interface Props {
  initialId?: string
  initialName?: string
  initialEnabled?: boolean
  initialConfig?: ProfileConfig
  isNew?: boolean
  onSubmit: (data: {
    id: string
    name: string
    enabled: boolean
    config: ProfileConfig
  }) => void
  onCancel: () => void
  loading?: boolean
}

const defaultConfig: ProfileConfig = {
  provider: 'anthropic',
  model: 'claude-sonnet-4-20250514',
  api_key_env: 'ANTHROPIC_API_KEY',
  channels: [],
  gateway: { max_history: 50 },
  env_vars: {},
}

export default function ProfileForm({
  initialId = '',
  initialName = '',
  initialEnabled = true,
  initialConfig,
  isNew = false,
  onSubmit,
  onCancel,
  loading = false,
}: Props) {
  const [id, setId] = useState(initialId)
  const [name, setName] = useState(initialName)
  const [enabled, setEnabled] = useState(initialEnabled)
  const [config, setConfig] = useState<ProfileConfig>(initialConfig || defaultConfig)
  const [activeTab, setActiveTab] = useState<'general' | 'llm' | 'env' | 'channels' | 'gateway'>('general')

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    onSubmit({ id, name, enabled, config })
  }

  const addChannel = (type: ChannelType) => {
    const channel = createDefaultChannel(type)
    setConfig({ ...config, channels: [...config.channels, channel] })
  }

  const removeChannel = (index: number) => {
    setConfig({ ...config, channels: config.channels.filter((_, i) => i !== index) })
  }

  const updateChannel = (index: number, updates: Record<string, string | number>) => {
    const channels = [...config.channels]
    channels[index] = { ...channels[index], ...updates }
    setConfig({ ...config, channels })
  }

  const addEnvVar = () => {
    setConfig({ ...config, env_vars: { ...config.env_vars, '': '' } })
  }

  const removeEnvVar = (key: string) => {
    const { [key]: _, ...rest } = config.env_vars
    setConfig({ ...config, env_vars: rest })
  }

  const updateEnvVar = (oldKey: string, newKey: string, value: string) => {
    const entries = Object.entries(config.env_vars).map(([k, v]) =>
      k === oldKey ? [newKey, value] : [k, v],
    )
    setConfig({ ...config, env_vars: Object.fromEntries(entries) })
  }

  const tabs = [
    { key: 'general' as const, label: 'General' },
    { key: 'llm' as const, label: 'LLM Provider' },
    { key: 'env' as const, label: 'API Keys' },
    { key: 'channels' as const, label: 'Channels' },
    { key: 'gateway' as const, label: 'Gateway' },
  ]

  return (
    <form onSubmit={handleSubmit}>
      {/* Tab navigation */}
      <div className="flex border-b border-gray-700/50 mb-6">
        {tabs.map((tab) => (
          <button
            key={tab.key}
            type="button"
            onClick={() => setActiveTab(tab.key)}
            className={`px-4 py-2.5 text-sm font-medium border-b-2 transition ${
              activeTab === tab.key
                ? 'border-accent text-accent'
                : 'border-transparent text-gray-500 hover:text-gray-300'
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* General Tab */}
      {activeTab === 'general' && (
        <div className="space-y-4">
          <Field label="Profile ID" hint="Lowercase letters, digits, hyphens. Cannot change after creation.">
            <input
              value={id}
              onChange={(e) => setId(e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, ''))}
              disabled={!isNew}
              placeholder="alice-bot"
              className="input"
              required
            />
          </Field>
          <Field label="Display Name">
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Alice's Bot"
              className="input"
              required
            />
          </Field>
          <Field label="Auto-start">
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
                className="w-4 h-4 rounded bg-surface-dark border-gray-600 text-accent focus:ring-accent"
              />
              <span className="text-sm text-gray-400">Start gateway automatically when server starts</span>
            </label>
          </Field>
        </div>
      )}

      {/* LLM Tab */}
      {activeTab === 'llm' && (
        <div className="space-y-4">
          <Field label="Provider">
            <select
              value={config.provider || ''}
              onChange={(e) => {
                const provider = e.target.value || null
                setConfig({ ...config, provider })
              }}
              className="input"
            >
              <option value="">Auto-detect</option>
              {PROVIDERS.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Model">
            <input
              value={config.model || ''}
              onChange={(e) => setConfig({ ...config, model: e.target.value || null })}
              placeholder={PROVIDER_DEFAULTS[config.provider || '']?.model || 'claude-sonnet-4-20250514'}
              className="input"
            />
          </Field>
          <Field label="API Key" hint="The actual API key / secret for this provider. Stored securely and passed to the gateway process.">
            <input
              type="password"
              value={config.env_vars[getApiKeyEnvName(config.provider)] || ''}
              onChange={(e) => {
                const envName = getApiKeyEnvName(config.provider)
                const newEnvVars = { ...config.env_vars }
                if (e.target.value) {
                  newEnvVars[envName] = e.target.value
                } else {
                  delete newEnvVars[envName]
                }
                setConfig({ ...config, api_key_env: envName, env_vars: newEnvVars })
              }}
              placeholder={`Paste your ${config.provider || 'anthropic'} API key`}
              className="input font-mono text-xs"
            />
            <p className="text-[10px] text-gray-600 mt-1">
              Will be set as <code className="text-gray-500">{getApiKeyEnvName(config.provider)}</code>
            </p>
          </Field>
        </div>
      )}

      {/* API Keys / Env Vars Tab */}
      {activeTab === 'env' && (
        <div className="space-y-4">
          <p className="text-xs text-gray-500">
            Set environment variables that will be passed to the gateway process.
            Use this to provide API keys, tokens, and other secrets.
          </p>

          {Object.entries(config.env_vars).map(([key, value], i) => (
            <div key={i} className="flex gap-2 items-start">
              <div className="flex-1">
                <input
                  value={key}
                  onChange={(e) => updateEnvVar(key, e.target.value, value)}
                  placeholder="ANTHROPIC_API_KEY"
                  className="input text-xs font-mono"
                />
              </div>
              <div className="flex-[2]">
                <input
                  type="password"
                  value={value}
                  onChange={(e) => updateEnvVar(key, key, e.target.value)}
                  placeholder="sk-ant-..."
                  className="input text-xs font-mono"
                />
              </div>
              <button
                type="button"
                onClick={() => removeEnvVar(key)}
                className="px-2 py-2 text-xs text-red-400 hover:text-red-300"
              >
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            </div>
          ))}

          <button
            type="button"
            onClick={addEnvVar}
            className="px-3 py-1.5 text-xs font-medium rounded-lg bg-white/5 text-gray-400 hover:bg-white/10 hover:text-white border border-gray-700/50 transition"
          >
            + Add Environment Variable
          </button>

          <div className="bg-surface-dark rounded-lg border border-gray-700/50 p-4 mt-4">
            <h4 className="text-xs font-semibold text-gray-400 mb-2">Common API Keys</h4>
            <div className="flex flex-wrap gap-1.5">
              {[
                'ANTHROPIC_API_KEY',
                'OPENAI_API_KEY',
                'GEMINI_API_KEY',
                'OPENROUTER_API_KEY',
                'DEEPSEEK_API_KEY',
                'GROQ_API_KEY',
                'MOONSHOT_API_KEY',
                'MINIMAX_API_KEY',
                'DASHSCOPE_API_KEY',
                'ZHIPU_API_KEY',
                'GLM_API_KEY',
                'TELEGRAM_BOT_TOKEN',
                'DISCORD_BOT_TOKEN',
                'SLACK_BOT_TOKEN',
                'SLACK_APP_TOKEN',
              ]
                .filter((k) => !(k in config.env_vars))
                .map((k) => (
                  <button
                    key={k}
                    type="button"
                    onClick={() => setConfig({ ...config, env_vars: { ...config.env_vars, [k]: '' } })}
                    className="px-2 py-1 text-[10px] font-mono rounded bg-white/5 text-gray-500 hover:text-gray-300 hover:bg-white/10 transition"
                  >
                    {k}
                  </button>
                ))}
            </div>
          </div>
        </div>
      )}

      {/* Channels Tab */}
      {activeTab === 'channels' && (
        <div className="space-y-4">
          {config.channels.map((ch, i) => (
            <ChannelEditor
              key={i}
              channel={ch}
              onChange={(updates) => updateChannel(i, updates)}
              onRemove={() => removeChannel(i)}
            />
          ))}

          <div className="flex flex-wrap gap-2">
            {CHANNEL_TYPES.filter(
              (t) => !config.channels.some((c) => c.type === t),
            ).map((type) => (
              <button
                key={type}
                type="button"
                onClick={() => addChannel(type)}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-white/5 text-gray-400 hover:bg-white/10 hover:text-white border border-gray-700/50 transition"
              >
                + {type}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Gateway Tab */}
      {activeTab === 'gateway' && (
        <div className="space-y-4">
          <Field label="Max History">
            <input
              type="number"
              value={config.gateway.max_history ?? ''}
              onChange={(e) =>
                setConfig({
                  ...config,
                  gateway: {
                    ...config.gateway,
                    max_history: e.target.value ? Number(e.target.value) : null,
                  },
                })
              }
              placeholder="50"
              className="input"
            />
          </Field>
          <Field label="Max Iterations">
            <input
              type="number"
              value={config.gateway.max_iterations ?? ''}
              onChange={(e) =>
                setConfig({
                  ...config,
                  gateway: {
                    ...config.gateway,
                    max_iterations: e.target.value ? Number(e.target.value) : null,
                  },
                })
              }
              placeholder="50"
              className="input"
            />
          </Field>
          <Field label="System Prompt">
            <textarea
              value={config.gateway.system_prompt ?? ''}
              onChange={(e) =>
                setConfig({
                  ...config,
                  gateway: {
                    ...config.gateway,
                    system_prompt: e.target.value || null,
                  },
                })
              }
              placeholder="You are a helpful assistant."
              rows={4}
              className="input"
            />
          </Field>
        </div>
      )}

      {/* Actions */}
      <div className="flex justify-end gap-3 mt-8 pt-4 border-t border-gray-700/50">
        <button
          type="button"
          onClick={onCancel}
          className="px-4 py-2 text-sm font-medium text-gray-400 hover:text-white rounded-lg hover:bg-white/5 transition"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={loading}
          className="px-6 py-2 text-sm font-medium rounded-lg bg-accent text-white hover:bg-accent-light transition disabled:opacity-50"
        >
          {loading ? 'Saving...' : isNew ? 'Create Profile' : 'Save Changes'}
        </button>
      </div>
    </form>
  )
}

function Field({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div>
      <label className="block text-sm font-medium text-gray-300 mb-1.5">{label}</label>
      {hint && <p className="text-xs text-gray-500 mb-1.5">{hint}</p>}
      {children}
    </div>
  )
}

function ChannelEditor({
  channel,
  onChange,
  onRemove,
}: {
  channel: ChannelCredentials
  onChange: (updates: Record<string, string | number>) => void
  onRemove: () => void
}) {
  const type = channel.type

  return (
    <div className="bg-surface-dark rounded-lg border border-gray-700/50 p-4">
      <div className="flex items-center justify-between mb-3">
        <span className="text-sm font-medium text-white capitalize">{type}</span>
        <button
          type="button"
          onClick={onRemove}
          className="text-xs text-red-400 hover:text-red-300"
        >
          Remove
        </button>
      </div>
      <div className="space-y-3">
        {type === 'telegram' && (
          <SmallField label="Token Env" value={channel.token_env as string || ''} onChange={(v) => onChange({ token_env: v })} placeholder="TELEGRAM_BOT_TOKEN" />
        )}
        {type === 'discord' && (
          <SmallField label="Token Env" value={channel.token_env as string || ''} onChange={(v) => onChange({ token_env: v })} placeholder="DISCORD_BOT_TOKEN" />
        )}
        {type === 'slack' && (
          <>
            <SmallField label="Bot Token Env" value={channel.bot_token_env as string || ''} onChange={(v) => onChange({ bot_token_env: v })} placeholder="SLACK_BOT_TOKEN" />
            <SmallField label="App Token Env" value={channel.app_token_env as string || ''} onChange={(v) => onChange({ app_token_env: v })} placeholder="SLACK_APP_TOKEN" />
          </>
        )}
        {type === 'whatsapp' && (
          <SmallField label="Bridge URL" value={channel.bridge_url as string || ''} onChange={(v) => onChange({ bridge_url: v })} placeholder="ws://localhost:3001" />
        )}
        {type === 'feishu' && (
          <>
            <SmallField label="App ID Env" value={channel.app_id_env as string || ''} onChange={(v) => onChange({ app_id_env: v })} placeholder="FEISHU_APP_ID" />
            <SmallField label="App Secret Env" value={channel.app_secret_env as string || ''} onChange={(v) => onChange({ app_secret_env: v })} placeholder="FEISHU_APP_SECRET" />
          </>
        )}
        {type === 'email' && (
          <>
            <SmallField label="IMAP Host" value={channel.imap_host as string || ''} onChange={(v) => onChange({ imap_host: v })} placeholder="imap.gmail.com" />
            <SmallField label="SMTP Host" value={channel.smtp_host as string || ''} onChange={(v) => onChange({ smtp_host: v })} placeholder="smtp.gmail.com" />
            <SmallField label="Username Env" value={channel.username_env as string || ''} onChange={(v) => onChange({ username_env: v })} placeholder="EMAIL_USERNAME" />
            <SmallField label="Password Env" value={channel.password_env as string || ''} onChange={(v) => onChange({ password_env: v })} placeholder="EMAIL_PASSWORD" />
          </>
        )}
      </div>
    </div>
  )
}

function SmallField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string
  value: string
  onChange: (v: string) => void
  placeholder: string
}) {
  return (
    <div>
      <label className="block text-xs text-gray-500 mb-1">{label}</label>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="input text-xs"
      />
    </div>
  )
}

function createDefaultChannel(type: ChannelType): ChannelCredentials {
  switch (type) {
    case 'telegram':
      return { type: 'telegram', token_env: 'TELEGRAM_BOT_TOKEN' }
    case 'discord':
      return { type: 'discord', token_env: 'DISCORD_BOT_TOKEN' }
    case 'slack':
      return { type: 'slack', bot_token_env: 'SLACK_BOT_TOKEN', app_token_env: 'SLACK_APP_TOKEN' }
    case 'whatsapp':
      return { type: 'whatsapp', bridge_url: 'ws://localhost:3001' }
    case 'feishu':
      return { type: 'feishu', app_id_env: 'FEISHU_APP_ID', app_secret_env: 'FEISHU_APP_SECRET' }
    case 'email':
      return {
        type: 'email',
        imap_host: '',
        imap_port: 993,
        smtp_host: '',
        smtp_port: 465,
        username_env: 'EMAIL_USERNAME',
        password_env: 'EMAIL_PASSWORD',
      }
  }
}

const PROVIDER_DEFAULTS: Record<string, { env: string; model: string }> = {
  anthropic: { env: 'ANTHROPIC_API_KEY', model: 'claude-sonnet-4-20250514' },
  openai: { env: 'OPENAI_API_KEY', model: 'gpt-4o' },
  gemini: { env: 'GEMINI_API_KEY', model: 'gemini-2.0-flash' },
  openrouter: { env: 'OPENROUTER_API_KEY', model: 'anthropic/claude-sonnet-4-20250514' },
  deepseek: { env: 'DEEPSEEK_API_KEY', model: 'deepseek-chat' },
  groq: { env: 'GROQ_API_KEY', model: 'llama-3.3-70b-versatile' },
  moonshot: { env: 'MOONSHOT_API_KEY', model: 'kimi-k2.5' },
  dashscope: { env: 'DASHSCOPE_API_KEY', model: 'qwen-max' },
  minimax: { env: 'MINIMAX_API_KEY', model: 'MiniMax-Text-01' },
  zhipu: { env: 'ZHIPU_API_KEY', model: 'glm-4-plus' },
  glm: { env: 'GLM_API_KEY', model: 'glm-5.0' },
  ollama: { env: '', model: 'llama3.2' },
  vllm: { env: 'VLLM_API_KEY', model: '' },
}

function getApiKeyEnvName(provider: string | null | undefined): string {
  return PROVIDER_DEFAULTS[provider || 'anthropic']?.env || `${(provider || 'ANTHROPIC').toUpperCase()}_API_KEY`
}
