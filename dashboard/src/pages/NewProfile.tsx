import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useToast } from '../components/Toast'
import { api } from '../api'
import ProfileForm from '../components/ProfileForm'

export default function NewProfile() {
  const navigate = useNavigate()
  const { toast } = useToast()
  const [loading, setLoading] = useState(false)

  const handleSubmit = async (data: any) => {
    try {
      setLoading(true)
      await api.createProfile({
        id: data.id,
        name: data.name,
        enabled: data.enabled,
        config: data.config,
      })
      toast('Profile created')
      navigate('/')
    } catch (e: any) {
      toast(e.message, 'error')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div>
      <div className="mb-6">
        <button
          onClick={() => navigate('/')}
          className="text-sm text-gray-500 hover:text-gray-300 mb-2 flex items-center gap-1"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
          </svg>
          Back
        </button>
        <h1 className="text-2xl font-bold text-white">New Profile</h1>
        <p className="text-sm text-gray-500 mt-1">
          Create a new user profile with LLM provider and channel configuration.
        </p>
      </div>

      <div className="bg-surface rounded-xl border border-gray-700/50 p-6">
        <ProfileForm
          isNew
          onSubmit={handleSubmit}
          onCancel={() => navigate('/')}
          loading={loading}
        />
      </div>
    </div>
  )
}
