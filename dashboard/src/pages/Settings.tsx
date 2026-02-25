import { useState, useEffect } from 'react'

export default function Settings() {
  const [token, setToken] = useState('')

  useEffect(() => {
    setToken(localStorage.getItem('crew_auth_token') || '')
  }, [])

  const saveToken = () => {
    if (token) {
      localStorage.setItem('crew_auth_token', token)
    } else {
      localStorage.removeItem('crew_auth_token')
    }
  }

  return (
    <div>
      <div className="mb-6">
        <h1 className="text-2xl font-bold text-white">Settings</h1>
        <p className="text-sm text-gray-500 mt-1">
          Configure dashboard preferences and authentication.
        </p>
      </div>

      <div className="bg-surface rounded-xl border border-gray-700/50 p-6 max-w-lg">
        <h3 className="text-sm font-semibold text-white mb-4">Authentication</h3>
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-gray-300 mb-1.5">
              API Auth Token
            </label>
            <p className="text-xs text-gray-500 mb-2">
              Bearer token for authenticating with the crew-rs API. Set via{' '}
              <code className="text-accent">--auth-token</code> on the server.
            </p>
            <div className="flex gap-2">
              <input
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="Enter auth token..."
                className="input flex-1"
              />
              <button
                onClick={saveToken}
                className="px-4 py-2 text-sm font-medium rounded-lg bg-accent text-white hover:bg-accent-light transition"
              >
                Save
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
