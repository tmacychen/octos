import { useState } from 'react'
import { useParams, useNavigate } from 'react-router-dom'
import { useProfile } from '../hooks/useProfiles'
import { useToast } from '../components/Toast'
import { api } from '../api'
import ProfileForm from '../components/ProfileForm'
import GatewayControls from '../components/GatewayControls'
import LogViewer from '../components/LogViewer'
import ConfirmDialog from '../components/ConfirmDialog'

export default function ProfilePage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()
  const { profile, error, loading, refresh } = useProfile(id)
  const { toast } = useToast()
  const [activeTab, setActiveTab] = useState<'overview' | 'config' | 'logs'>('overview')
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [actionLoading, setActionLoading] = useState(false)
  const [saving, setSaving] = useState(false)

  if (loading && !profile) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="animate-spin w-6 h-6 border-2 border-accent border-t-transparent rounded-full" />
      </div>
    )
  }

  if (error || !profile) {
    return (
      <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4 text-red-400 text-sm">
        {error || 'Profile not found'}
      </div>
    )
  }

  const handleStart = async () => {
    try {
      setActionLoading(true)
      await api.startGateway(profile.id)
      toast('Gateway started')
      refresh()
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setActionLoading(false)
    }
  }

  const handleStop = async () => {
    try {
      setActionLoading(true)
      await api.stopGateway(profile.id)
      toast('Gateway stopped')
      refresh()
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setActionLoading(false)
    }
  }

  const handleRestart = async () => {
    try {
      setActionLoading(true)
      await api.restartGateway(profile.id)
      toast('Gateway restarted')
      refresh()
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setActionLoading(false)
    }
  }

  const handleDelete = async () => {
    try {
      await api.deleteProfile(profile.id)
      toast('Profile deleted')
      navigate('/')
    } catch (e: any) {
      toast(e.message, 'error')
    }
    setDeleteOpen(false)
  }

  const handleSave = async (data: any) => {
    try {
      setSaving(true)
      await api.updateProfile(profile.id, {
        name: data.name,
        enabled: data.enabled,
        config: data.config,
      })
      toast('Profile saved')
      refresh()
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setSaving(false)
    }
  }

  const tabs = [
    { key: 'overview' as const, label: 'Overview' },
    { key: 'config' as const, label: 'Configuration' },
    { key: 'logs' as const, label: 'Logs' },
  ]

  return (
    <div>
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <button
            onClick={() => navigate('/')}
            className="text-sm text-gray-500 hover:text-gray-300 mb-2 flex items-center gap-1"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
            </svg>
            Back
          </button>
          <h1 className="text-2xl font-bold text-white">{profile.name}</h1>
          <p className="text-sm text-gray-500 mt-1">
            {profile.id} &middot; {profile.config.provider || 'anthropic'} &middot;{' '}
            {profile.config.model || 'default'}
          </p>
        </div>
        <button
          onClick={() => setDeleteOpen(true)}
          className="px-4 py-2 text-sm font-medium rounded-lg bg-red-500/10 text-red-400 hover:bg-red-500/20 border border-red-500/20 transition"
        >
          Delete
        </button>
      </div>

      {/* Tabs */}
      <div className="flex border-b border-gray-700/50 mb-6">
        {tabs.map((tab) => (
          <button
            key={tab.key}
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

      {/* Overview Tab */}
      {activeTab === 'overview' && (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          <GatewayControls
            status={profile.status}
            loading={actionLoading}
            onStart={handleStart}
            onStop={handleStop}
            onRestart={handleRestart}
          />
          <div className="bg-surface rounded-xl border border-gray-700/50 p-5">
            <h3 className="text-sm font-semibold text-white mb-4">Profile Info</h3>
            <dl className="space-y-3 text-xs">
              <InfoRow label="ID" value={profile.id} />
              <InfoRow label="Provider" value={profile.config.provider || 'anthropic'} />
              <InfoRow label="Model" value={profile.config.model || 'default'} />
              <InfoRow label="Channels" value={profile.config.channels.map((c) => c.type).join(', ') || 'None'} />
              <InfoRow label="Auto-start" value={profile.enabled ? 'Yes' : 'No'} />
              <InfoRow label="Created" value={new Date(profile.created_at).toLocaleDateString()} />
              <InfoRow label="Updated" value={new Date(profile.updated_at).toLocaleDateString()} />
            </dl>
          </div>
        </div>
      )}

      {/* Config Tab */}
      {activeTab === 'config' && (
        <div className="bg-surface rounded-xl border border-gray-700/50 p-6">
          <ProfileForm
            initialId={profile.id}
            initialName={profile.name}
            initialEnabled={profile.enabled}
            initialConfig={profile.config}
            onSubmit={handleSave}
            onCancel={() => setActiveTab('overview')}
            loading={saving}
          />
        </div>
      )}

      {/* Logs Tab */}
      {activeTab === 'logs' && <LogViewer profileId={profile.id} />}

      <ConfirmDialog
        open={deleteOpen}
        title="Delete Profile"
        message={`Are you sure you want to delete "${profile.name}"? This will stop the gateway and remove all configuration.`}
        confirmLabel="Delete"
        danger
        onConfirm={handleDelete}
        onCancel={() => setDeleteOpen(false)}
      />
    </div>
  )
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between">
      <dt className="text-gray-500">{label}</dt>
      <dd className="text-gray-300">{value}</dd>
    </div>
  )
}
