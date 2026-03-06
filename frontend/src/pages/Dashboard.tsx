import { useQuery } from '@tanstack/react-query'
import { api, type Stats } from '../api'
import { useEffect, useRef, useState } from 'react'

function useCounter(target: number, duration = 1200) {
  const [val, setVal] = useState(0)
  const rafRef = useRef<number>(0)
  useEffect(() => {
    const start = performance.now()
    const tick = (now: number) => {
      const p = Math.min((now - start) / duration, 1)
      const ease = 1 - Math.pow(1 - p, 3)
      setVal(Math.round(ease * target))
      if (p < 1) rafRef.current = requestAnimationFrame(tick)
    }
    rafRef.current = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(rafRef.current)
  }, [target, duration])
  return val
}

function StatCard({ label, value, gold, delay = 0 }: {
  label: string; value: number; gold?: boolean; delay?: number
}) {
  const count = useCounter(value, 1000)
  return (
    <div className={`stat-card${gold ? ' card--gold' : ''}`}
      style={{ animationDelay: `${delay}ms`, animation: 'fadeUp 0.4s ease both' }}>
      <div className="stat-label">{label}</div>
      <div className={`stat-value${gold ? '' : ' stat-value--dim'}`}>
        {count.toLocaleString()}
      </div>
    </div>
  )
}

export default function Dashboard() {
  const { data, isLoading, error } = useQuery<Stats>({
    queryKey: ['stats'],
    queryFn: api.stats,
    refetchInterval: 10_000,
  })

  const { data: jobs } = useQuery({
    queryKey: ['discovery-jobs'],
    queryFn: api.discoveryJobs,
    refetchInterval: 5000,
  })
  const activeJob = jobs?.find(j => j.status === 'running')

  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">DASHBOARD</h1>
        <p className="page-subtitle">Dutch music preservation overview - live from the archive</p>
      </div>

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">Cannot reach API - is mm-api running?</div>}

      {data && (
        <>
          <div className="stat-grid">
            <StatCard label="Total releases" value={data.total_releases} gold delay={0} />
            <StatCard label="Missing from streaming" value={data.missing_from_streaming} gold delay={60} />
            <StatCard label="Public domain" value={data.public_domain} delay={120} />
            <StatCard label="On watchlist" value={data.watchlist_total} delay={180} />
            <StatCard label="To buy" value={data.watchlist_to_buy} delay={240} />
            <StatCard label="Digitized" value={data.watchlist_done} delay={300} />
            <StatCard label="Tracks ripped" value={data.tracks_digitized} gold delay={360} />
            <StatCard label="Rip jobs done" value={data.rip_jobs_done} delay={420} />
          </div>

          {activeJob && (
            <div className="card" style={{ marginBottom: 14, display: 'flex', alignItems: 'center', gap: 12 }}>
              <span style={{ width: 8, height: 8, borderRadius: '50%', background: '#facc15', boxShadow: '0 0 8px #facc1588', flexShrink: 0 }} />
              <span style={{ fontSize: 13 }}>
                Job running: <span className="mono" style={{ color: 'var(--gold)' }}>{activeJob.country} {activeJob.year_from}–{activeJob.year_to}</span>
                {' - '}{activeJob.current_page}/{activeJob.total_pages ?? '?'} pages · {activeJob.releases_saved} saved
              </span>
              <a href="/jobs" style={{ marginLeft: 'auto', fontSize: 12, color: 'var(--gold)' }}>View jobs →</a>
            </div>
          )}

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 14 }}>
            <div className="card">
              <div style={{ fontFamily: 'var(--font-display)', fontSize: 18, letterSpacing: '0.06em', marginBottom: 14, color: 'var(--gold)' }}>
                WORKFLOW PIPELINE
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                {[
                  { label: 'Search Discogs', done: data.total_releases > 0 },
                  { label: 'Platform checks run', done: data.missing_from_streaming > 0 },
                  { label: 'Items on watchlist', done: data.watchlist_total > 0 },
                  { label: 'Ready to rip', done: data.watchlist_to_buy < data.watchlist_total },
                  { label: 'Tracks digitized', done: data.tracks_digitized > 0 },
                ].map(({ label, done }) => (
                  <div key={label} style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                    <span style={{
                      width: 8, height: 8, borderRadius: '50%', flexShrink: 0,
                      background: done ? '#4ade80' : 'var(--bg-hover)',
                      border: done ? 'none' : '1px solid var(--border)',
                      boxShadow: done ? '0 0 8px #4ade8055' : 'none',
                    }} />
                    <span style={{ fontSize: 13, color: done ? 'var(--text-primary)' : 'var(--text-muted)' }}>
                      {label}
                    </span>
                  </div>
                ))}
              </div>
            </div>

            <div className="card">
              <div style={{ fontFamily: 'var(--font-display)', fontSize: 18, letterSpacing: '0.06em', marginBottom: 14, color: 'var(--gold)' }}>
                QUICK ACTIONS
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                {[
                  { cmd: 'mm search --auto-watchlist', desc: 'Discover NL releases (CLI)' },
                  { cmd: 'mm worker --workers 2', desc: 'Check all platforms' },
                  { cmd: 'mm rip daemon', desc: 'Auto-rip on CD insert' },
                  { cmd: 'mm stats', desc: 'Show summary' },
                ].map(({ cmd, desc }) => (
                  <div key={cmd} style={{ background: 'var(--bg-raised)', borderRadius: 6, padding: '8px 12px' }}>
                    <div className="mono" style={{ fontSize: 12, color: 'var(--gold)', marginBottom: 2 }}>{cmd}</div>
                    <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>{desc}</div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  )
}
