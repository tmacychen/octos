import { Routes, Route } from 'react-router-dom'
import { ToastProvider } from './components/Toast'
import Layout from './components/Layout'
import Dashboard from './pages/Dashboard'
import ProfilePage from './pages/ProfilePage'
import NewProfile from './pages/NewProfile'
import Settings from './pages/Settings'

export default function App() {
  return (
    <ToastProvider>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route path="profiles/new" element={<NewProfile />} />
          <Route path="profiles/:id" element={<ProfilePage />} />
          <Route path="settings" element={<Settings />} />
        </Route>
      </Routes>
    </ToastProvider>
  )
}
