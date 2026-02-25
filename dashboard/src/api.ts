import type {
  ProfileResponse,
  OverviewResponse,
  ActionResponse,
  BulkActionResponse,
  ProfileConfig,
} from './types'

const BASE = '/api/admin'

function getHeaders(): HeadersInit {
  const headers: HeadersInit = { 'Content-Type': 'application/json' }
  const token = localStorage.getItem('crew_auth_token')
  if (token) {
    headers['Authorization'] = `Bearer ${token}`
  }
  return headers
}

async function request<T>(path: string, opts?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    headers: getHeaders(),
    ...opts,
  })
  if (!res.ok) {
    const text = await res.text()
    throw new Error(text || `HTTP ${res.status}`)
  }
  return res.json()
}

export const api = {
  overview: () => request<OverviewResponse>('/overview'),

  listProfiles: () => request<ProfileResponse[]>('/profiles'),

  getProfile: (id: string) => request<ProfileResponse>(`/profiles/${id}`),

  createProfile: (data: {
    id: string
    name: string
    enabled?: boolean
    data_dir?: string | null
    config?: ProfileConfig
  }) =>
    request<ProfileResponse>('/profiles', {
      method: 'POST',
      body: JSON.stringify(data),
    }),

  updateProfile: (
    id: string,
    data: {
      name?: string
      enabled?: boolean
      data_dir?: string | null
      config?: ProfileConfig
    },
  ) =>
    request<ProfileResponse>(`/profiles/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    }),

  deleteProfile: (id: string) =>
    request<ActionResponse>(`/profiles/${id}`, { method: 'DELETE' }),

  startGateway: (id: string) =>
    request<ActionResponse>(`/profiles/${id}/start`, { method: 'POST' }),

  stopGateway: (id: string) =>
    request<ActionResponse>(`/profiles/${id}/stop`, { method: 'POST' }),

  restartGateway: (id: string) =>
    request<ActionResponse>(`/profiles/${id}/restart`, { method: 'POST' }),

  startAll: () => request<BulkActionResponse>('/start-all', { method: 'POST' }),

  stopAll: () => request<BulkActionResponse>('/stop-all', { method: 'POST' }),
}
