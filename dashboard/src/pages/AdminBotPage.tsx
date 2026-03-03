import { useState, useEffect, useCallback } from 'react'
import { api } from '../api'
import { useToast } from '../components/Toast'
import type { AdminBotConfig } from '../types'
import { PROVIDERS } from '../types'

const DEFAULT_CONFIG: AdminBotConfig = {
  telegram_token_env: null,
  feishu_app_id_env: null,
  feishu_app_secret_env: null,
  admin_chat_ids: [],
  admin_feishu_ids: [],
  provider: null,
  model: null,
  base_url: null,
  api_key_env: null,
  alerts_enabled: true,
  watchdog_enabled: true,
  health_check_interval_secs: 60,
  max_restart_attempts: 3,
}

export default function AdminBotPage() {
  const { toast } = useToast()
  const [config, setConfig] = useState<AdminBotConfig>(DEFAULT_CONFIG)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [dirty, setDirty] = useState(false)

  // Temp inputs for adding list items
  const [newChatId, setNewChatId] = useState('')
  const [newFeishuId, setNewFeishuId] = useState('')

  const loadConfig = useCallback(async () => {
    try {
      const data = await api.getAdminBot()
      setConfig(data)
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setLoading(false)
    }
  }, [toast])

  useEffect(() => {
    loadConfig()
  }, [loadConfig])

  const update = (patch: Partial<AdminBotConfig>) => {
    setConfig((prev) => ({ ...prev, ...patch }))
    setDirty(true)
  }

  const handleSave = async () => {
    try {
      setSaving(true)
      const updated = await api.updateAdminBot(config)
      setConfig(updated)
      setDirty(false)
      toast('Admin bot config saved')
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setSaving(false)
    }
  }

  const addChatId = () => {
    const id = parseInt(newChatId, 10)
    if (isNaN(id)) return
    if (config.admin_chat_ids.includes(id)) return
    update({ admin_chat_ids: [...config.admin_chat_ids, id] })
    setNewChatId('')
  }

  const removeChatId = (id: number) => {
    update({ admin_chat_ids: config.admin_chat_ids.filter((x) => x !== id) })
  }

  const addFeishuId = () => {
    const id = newFeishuId.trim()
    if (!id) return
    if (config.admin_feishu_ids.includes(id)) return
    update({ admin_feishu_ids: [...config.admin_feishu_ids, id] })
    setNewFeishuId('')
  }

  const removeFeishuId = (id: string) => {
    update({ admin_feishu_ids: config.admin_feishu_ids.filter((x) => x !== id) })
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="animate-spin w-6 h-6 border-2 border-accent border-t-transparent rounded-full" />
      </div>
    )
  }

  return (
    <div className="max-w-3xl">
      {/* Header */}
      <div className="mb-6">
        <h1 className="text-2xl font-bold text-white">Admin Bot</h1>
        <p className="text-sm text-gray-500 mt-1">
          Configure the LLM-powered admin bot for Telegram and Feishu
        </p>
      </div>

      {/* Section 1: Messaging Channels */}
      <Section title="Messaging Channels">
        {/* Telegram */}
        <SubSection title="Telegram">
          <Field label="Token env var">
            <input
              className="input text-sm"
              placeholder="ADMIN_BOT_TOKEN"
              value={config.telegram_token_env || ''}
              onChange={(e) => update({ telegram_token_env: e.target.value || null })}
            />
          </Field>
          <Field label="Admin chat IDs">
            <div className="space-y-2">
              <div className="flex gap-2">
                <input
                  className="input text-sm flex-1"
                  placeholder="Chat ID (e.g. 123456789)"
                  value={newChatId}
                  onChange={(e) => setNewChatId(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && (e.preventDefault(), addChatId())}
                />
                <button
                  type="button"
                  onClick={addChatId}
                  className="px-3 py-1.5 text-xs font-medium rounded-lg bg-white/5 text-gray-300 hover:bg-white/10 transition"
                >
                  Add
                </button>
              </div>
              {config.admin_chat_ids.length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {config.admin_chat_ids.map((id) => (
                    <Tag key={id} onRemove={() => removeChatId(id)}>
                      {id}
                    </Tag>
                  ))}
                </div>
              )}
            </div>
          </Field>
        </SubSection>

        {/* Feishu */}
        <SubSection title="Feishu / Lark">
          <Field label="App ID env var">
            <input
              className="input text-sm"
              placeholder="ADMIN_FEISHU_APP_ID"
              value={config.feishu_app_id_env || ''}
              onChange={(e) => update({ feishu_app_id_env: e.target.value || null })}
            />
          </Field>
          <Field label="App secret env var">
            <input
              className="input text-sm"
              placeholder="ADMIN_FEISHU_APP_SECRET"
              value={config.feishu_app_secret_env || ''}
              onChange={(e) => update({ feishu_app_secret_env: e.target.value || null })}
            />
          </Field>
          <Field label="Admin user IDs">
            <div className="space-y-2">
              <div className="flex gap-2">
                <input
                  className="input text-sm flex-1"
                  placeholder="Feishu user ID"
                  value={newFeishuId}
                  onChange={(e) => setNewFeishuId(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && (e.preventDefault(), addFeishuId())}
                />
                <button
                  type="button"
                  onClick={addFeishuId}
                  className="px-3 py-1.5 text-xs font-medium rounded-lg bg-white/5 text-gray-300 hover:bg-white/10 transition"
                >
                  Add
                </button>
              </div>
              {config.admin_feishu_ids.length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {config.admin_feishu_ids.map((id) => (
                    <Tag key={id} onRemove={() => removeFeishuId(id)}>
                      {id}
                    </Tag>
                  ))}
                </div>
              )}
            </div>
          </Field>
        </SubSection>
      </Section>

      {/* Section 2: LLM Provider */}
      <Section title="LLM Provider">
        <Field label="Provider">
          <select
            className="input text-sm"
            value={config.provider || ''}
            onChange={(e) => update({ provider: e.target.value || null })}
          >
            <option value="">— inherit from global —</option>
            {PROVIDERS.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Model">
          <input
            className="input text-sm"
            placeholder="e.g. gpt-4o-mini"
            value={config.model || ''}
            onChange={(e) => update({ model: e.target.value || null })}
          />
        </Field>
        <Field label="Base URL">
          <input
            className="input text-sm"
            placeholder="https://api.openai.com/v1"
            value={config.base_url || ''}
            onChange={(e) => update({ base_url: e.target.value || null })}
          />
        </Field>
        <Field label="API key env var">
          <input
            className="input text-sm"
            placeholder="OPENAI_API_KEY"
            value={config.api_key_env || ''}
            onChange={(e) => update({ api_key_env: e.target.value || null })}
          />
        </Field>
      </Section>

      {/* Section 3: Watchdog & Monitoring */}
      <Section title="Watchdog & Monitoring">
        <Toggle
          label="Alerts enabled"
          description="Send proactive alerts when gateways crash or become unhealthy"
          checked={config.alerts_enabled}
          onChange={(v) => update({ alerts_enabled: v })}
        />
        <Toggle
          label="Watchdog enabled"
          description="Automatically restart crashed gateways"
          checked={config.watchdog_enabled}
          onChange={(v) => update({ watchdog_enabled: v })}
        />
        <Field label="Health check interval (seconds)">
          <input
            type="number"
            className="input text-sm w-32"
            min={10}
            value={config.health_check_interval_secs}
            onChange={(e) =>
              update({ health_check_interval_secs: parseInt(e.target.value, 10) || 60 })
            }
          />
        </Field>
        <Field label="Max restart attempts">
          <input
            type="number"
            className="input text-sm w-32"
            min={0}
            value={config.max_restart_attempts}
            onChange={(e) =>
              update({ max_restart_attempts: parseInt(e.target.value, 10) || 3 })
            }
          />
        </Field>
      </Section>

      {/* Save footer */}
      {dirty && (
        <div className="sticky bottom-0 bg-bg/90 backdrop-blur border-t border-gray-700/50 py-4 -mx-6 px-6 flex items-center justify-between">
          <p className="text-xs text-yellow-400">Unsaved changes</p>
          <button
            onClick={handleSave}
            disabled={saving}
            className="px-5 py-2 text-sm font-medium rounded-lg bg-accent text-white hover:bg-accent-light transition disabled:opacity-50"
          >
            {saving ? 'Saving...' : 'Save Changes'}
          </button>
        </div>
      )}
    </div>
  )
}

// ── Reusable layout helpers ──

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="bg-surface rounded-xl border border-gray-700/50 p-5 mb-5">
      <h2 className="text-sm font-semibold text-white mb-4">{title}</h2>
      <div className="space-y-4">{children}</div>
    </div>
  )
}

function SubSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="space-y-3">
      <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wider">{title}</h3>
      {children}
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <label className="block text-xs text-gray-500 mb-1">{label}</label>
      {children}
    </div>
  )
}

function Toggle({
  label,
  description,
  checked,
  onChange,
}: {
  label: string
  description: string
  checked: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <div className="flex items-center justify-between">
      <div>
        <p className="text-sm text-white">{label}</p>
        <p className="text-xs text-gray-500">{description}</p>
      </div>
      <button
        type="button"
        onClick={() => onChange(!checked)}
        className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
          checked ? 'bg-accent' : 'bg-gray-600'
        }`}
      >
        <span
          className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${
            checked ? 'translate-x-4' : 'translate-x-0.5'
          }`}
        />
      </button>
    </div>
  )
}

function Tag({ children, onRemove }: { children: React.ReactNode; onRemove: () => void }) {
  return (
    <span className="inline-flex items-center gap-1 px-2 py-0.5 text-xs font-mono bg-white/5 text-gray-300 rounded-md">
      {children}
      <button
        type="button"
        onClick={onRemove}
        className="text-gray-500 hover:text-red-400 transition ml-0.5"
      >
        x
      </button>
    </span>
  )
}
