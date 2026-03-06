import { Routes, Route, NavLink } from 'react-router-dom'
import { LayoutDashboard, Disc3, ListChecks, Radio, FlaskConical } from 'lucide-react'
import Dashboard from './pages/Dashboard'
import Jobs from './pages/Jobs'
import Releases from './pages/Releases'
import Watchlist from './pages/Watchlist'
import RipJobs from './pages/RipJobs'
import './App.css'

const nav = [
  { to: '/',         icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/jobs',     icon: FlaskConical,    label: 'Jobs'      },
  { to: '/releases', icon: Disc3,           label: 'Releases'  },
  { to: '/watchlist',icon: ListChecks,      label: 'Watchlist' },
  { to: '/rip-jobs', icon: Radio,           label: 'Rip Jobs'  },
]

export default function App() {
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="sidebar-logo">
          <VinylIcon />
          <span className="sidebar-title">WAX<br/>VAULT</span>
        </div>
        <nav className="sidebar-nav">
          {nav.map(({ to, icon: Icon, label }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              className={({ isActive }) =>
                `nav-item${isActive ? ' nav-item--active' : ''}`
              }
            >
              <Icon size={16} strokeWidth={1.5} />
              <span>{label}</span>
            </NavLink>
          ))}
        </nav>
        <div className="sidebar-footer">
          <span className="mono" style={{ fontSize: 10, color: 'var(--text-muted)' }}>
            NL Music Archive
          </span>
        </div>
      </aside>
      <main className="main-content">
        <Routes>
          <Route path="/"          element={<Dashboard />} />
          <Route path="/jobs"      element={<Jobs />} />
          <Route path="/releases"  element={<Releases />} />
          <Route path="/watchlist" element={<Watchlist />} />
          <Route path="/rip-jobs"  element={<RipJobs />} />
        </Routes>
      </main>
    </div>
  )
}

function VinylIcon() {
  return (
    <svg width="32" height="32" viewBox="0 0 32 32" fill="none" className="vinyl-spin">
      <circle cx="16" cy="16" r="15" stroke="#F59E0B" strokeWidth="1.5" fill="none" />
      <circle cx="16" cy="16" r="10" stroke="#333" strokeWidth="5" fill="none" />
      <circle cx="16" cy="16" r="10" stroke="#F59E0B" strokeWidth="0.5" fill="none" strokeDasharray="3 4" />
      <circle cx="16" cy="16" r="2.5" fill="#F59E0B" />
    </svg>
  )
}
