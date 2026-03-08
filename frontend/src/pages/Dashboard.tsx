import { useQuery } from '@tanstack/react-query'
import { api, type Stats } from '../api'
import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'

function useCounter(target: number, duration = 1000) {
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

type StepStatus = 'done' | 'active' | 'waiting' | 'future'

function PipelineStep({
  number, title, description, status, count, total, action, onClick, note,
}: {
  number: number
  title: string
  description: string
  status: StepStatus
  count?: number
  total?: number
  action?: string
  onClick?: () => void
  note?: string
}) {
  const color = status === 'done' ? '#4ade80' : status === 'active' ? '#facc15' : '#334155'
  const textColor = status === 'done' ? '#4ade80' : status === 'active' ? '#facc15' : 'var(--text-muted)'
  const countDisplay = useCounter(count ?? 0)

  return (
    <div style={{
      display: 'flex', gap: 16, padding: '18px 20px',
      background: status === 'active' ? 'rgba(250,204,21,0.04)' : status === 'done' ? 'rgba(74,222,128,0.03)' : 'var(--bg-raised)',
      borderRadius: 10,
      border: `1px solid ${status === 'active' ? 'rgba(250,204,21,0.2)' : status === 'done' ? 'rgba(74,222,128,0.12)' : 'var(--border)'}`,
      opacity: status === 'future' ? 0.45 : 1,
      transition: 'all 0.2s',
    }}>
      {/* Step number */}
      <div style={{
        width: 36, height: 36, borderRadius: '50%', flexShrink: 0,
        background: status === 'done' ? '#4ade8022' : status === 'active' ? '#facc1522' : '#1e293b',
        border: `2px solid ${color}`,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        fontFamily: 'var(--font-display)', fontSize: 16, color,
        boxShadow: status === 'active' ? '0 0 12px #facc1533' : status === 'done' ? '0 0 8px #4ade8022' : 'none',
      }}>
        {status === 'done' ? '✓' : number}
      </div>

      {/* Content */}
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 10, marginBottom: 3 }}>
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 16, letterSpacing: '0.05em', color: textColor }}>
            {title}
          </span>
          {count !== undefined && (
            <span className="mono" style={{ fontSize: 13, color: status === 'done' ? '#4ade80' : status === 'active' ? '#facc15' : 'var(--text-muted)' }}>
              {countDisplay.toLocaleString()}{total !== undefined ? ` / ${total.toLocaleString()}` : ''}
            </span>
          )}
          {status === 'active' && (
            <span style={{ fontSize: 10, padding: '2px 7px', borderRadius: 99, background: '#facc1522', color: '#facc15', border: '1px solid #facc1533', fontWeight: 600, letterSpacing: '0.06em' }}>
              IN PROGRESS
            </span>
          )}
        </div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: note || action ? 8 : 0 }}>
          {description}
        </div>
        {note && (
          <div style={{ fontSize: 11, color: '#94a3b8', marginBottom: action ? 6 : 0 }}>
            {note}
          </div>
        )}
        {action && onClick && status !== 'future' && (
          <button onClick={onClick} style={{
            padding: '4px 12px', borderRadius: 5, fontSize: 11, fontWeight: 600,
            border: `1px solid ${status === 'active' ? '#facc1566' : '#4ade8066'}`,
            background: 'transparent',
            color: status === 'active' ? '#facc15' : '#4ade80',
            cursor: 'pointer', letterSpacing: '0.04em',
          }}>
            {action}
          </button>
        )}
      </div>

      {/* Progress bar for active steps */}
      {status === 'active' && count !== undefined && total !== undefined && total > 0 && (
        <div style={{ width: 80, display: 'flex', flexDirection: 'column', justifyContent: 'center', gap: 4 }}>
          <div style={{ height: 4, background: '#1e293b', borderRadius: 2, overflow: 'hidden' }}>
            <div style={{ height: '100%', width: `${Math.min(100, (count / total) * 100)}%`, background: '#facc15', borderRadius: 2 }} />
          </div>
          <div style={{ fontSize: 10, color: '#facc15', textAlign: 'right', fontFamily: 'var(--font-mono)' }}>
            {Math.round((count / total) * 100)}%
          </div>
        </div>
      )}
    </div>
  )
}

export default function Dashboard() {
  const navigate = useNavigate()
  const { data, isLoading, error } = useQuery<Stats>({
    queryKey: ['stats'],
    queryFn: api.stats,
    refetchInterval: 8000,
  })
  const { data: jobs } = useQuery({
    queryKey: ['discovery-jobs'],
    queryFn: api.discoveryJobs,
    refetchInterval: 5000,
  })
  const { data: checker } = useQuery({
    queryKey: ['platform-checker'],
    queryFn: api.platformCheckerStatus,
    refetchInterval: 5000,
  })

  const activeJob = jobs?.find(j => j.status === 'running')
  const totalReleases = data?.total_releases ?? 0
  const checkedCount = checker?.total_checked ?? 0
  const uncheckedCount = checker?.unchecked_count ?? 0
  const missingFromStreaming = data?.missing_from_streaming ?? 0
  const onWatchlist = data?.watchlist_total ?? 0
  const toBuy = data?.watchlist_to_buy ?? 0
  const tracksRipped = data?.tracks_digitized ?? 0
  const ripJobsDone = data?.rip_jobs_done ?? 0

  // Determine step statuses
  const s1: StepStatus = totalReleases > 0 ? 'done' : activeJob ? 'active' : 'active'
  const s2: StepStatus = checkedCount > 0 ? (uncheckedCount === 0 ? 'done' : 'active') : totalReleases > 0 ? 'active' : 'waiting'
  const s3: StepStatus = onWatchlist > 0 ? (toBuy === 0 ? 'done' : 'active') : missingFromStreaming > 0 ? 'active' : 'waiting'
  const s4: StepStatus = (data?.watchlist_done ?? 0) > 0 ? 'done' : onWatchlist > 0 ? 'active' : 'waiting'
  const s5: StepStatus = tracksRipped > 0 ? 'done' : (data?.watchlist_done ?? 0) > 0 ? 'active' : 'waiting'
  const s6: StepStatus = 'future'
  const s7: StepStatus = 'future'

  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">DASHBOARD</h1>
        <p className="page-subtitle">Dutch music preservation pipeline</p>
      </div>

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">Cannot reach API - is mm-api running?</div>}

      {data && (
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 340px', gap: 16, alignItems: 'start' }}>

          {/* Pipeline */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <PipelineStep
              number={1}
              title="Discover"
              description="Scan Discogs for Dutch albums not on streaming platforms"
              status={s1}
              count={totalReleases}
              action={activeJob ? "View job" : "Start job"}
              onClick={() => navigate('/jobs')}
              note={activeJob ? `Job running: page ${activeJob.current_page}/${activeJob.total_pages ?? '?'}` : undefined}
            />
            <PipelineStep
              number={2}
              title="Check Platforms"
              description="Verify each album track-by-track on Spotify, Deezer, Apple Music, Bandcamp"
              status={s2}
              count={checkedCount}
              total={totalReleases}
              action="View checker"
              onClick={() => navigate('/jobs')}
              note={uncheckedCount > 0 ? `${uncheckedCount} releases still to check` : undefined}
            />
            <PipelineStep
              number={3}
              title="Watchlist"
              description="Add albums confirmed missing from all platforms to your buy list"
              status={s3}
              count={onWatchlist}
              action="Browse missing"
              onClick={() => navigate('/releases')}
              note={missingFromStreaming > 0 ? `${missingFromStreaming} albums confirmed missing from streaming` : undefined}
            />
            <PipelineStep
              number={4}
              title="Buy Physical Media"
              description="Purchase vinyl/CD from Discogs marketplace, mark as ordered, then purchased"
              status={s4}
              count={toBuy}
              action="View watchlist"
              onClick={() => navigate('/watchlist')}
            />
            <PipelineStep
              number={5}
              title="Digitize"
              description="Insert CD/vinyl, auto-rip to MP3 320kbps, auto-tag with Discogs metadata"
              status={s5}
              count={tracksRipped}
              action="View rip jobs"
              onClick={() => navigate('/rip-jobs')}
              note={ripJobsDone > 0 ? `${ripJobsDone} rip jobs completed` : 'Connect CD drive and run: mm rip daemon'}
            />
            <PipelineStep
              number={6}
              title="Organize Library"
              description="Auto-create folders with album art from Discogs: Artist / Year - Album / tracks"
              status={s6}
              note="Coming soon"
            />
            <PipelineStep
              number={7}
              title="Upload to YouTube"
              description="Auto-upload digitized albums to your YouTube channel with cover art and metadata"
              status={s7}
              note="Coming soon"
            />
          </div>

          {/* Right column - stats summary */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {/* Overall progress */}
            <div className="card">
              <div style={{ fontFamily: 'var(--font-display)', fontSize: 16, letterSpacing: '0.06em', marginBottom: 14, color: 'var(--gold)' }}>
                PROGRESS
              </div>
              {[
                { label: 'Releases discovered', value: totalReleases, color: '#facc15' },
                { label: 'Platform checked', value: checkedCount, color: '#60a5fa' },
                { label: 'Missing from streaming', value: missingFromStreaming, color: '#f87171' },
                { label: 'On watchlist', value: onWatchlist, color: '#a78bfa' },
                { label: 'Tracks digitized', value: tracksRipped, color: '#4ade80' },
              ].map(({ label, value, color }) => {
                const count = value
                return (
                  <div key={label} style={{ marginBottom: 10 }}>
                    <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 4 }}>
                      <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>{label}</span>
                      <span className="mono" style={{ fontSize: 11, color }}>{count.toLocaleString()}</span>
                    </div>
                    {totalReleases > 0 && (
                      <div style={{ height: 3, background: 'var(--bg-hover)', borderRadius: 2, overflow: 'hidden' }}>
                        <div style={{
                          height: '100%', borderRadius: 2, background: color,
                          width: `${Math.min(100, (value / Math.max(totalReleases, 1)) * 100)}%`,
                          transition: 'width 0.5s ease',
                        }} />
                      </div>
                    )}
                  </div>
                )
              })}
            </div>

            {/* Active job ticker */}
            {(activeJob || checker?.active) && (
              <div className="card" style={{ borderColor: 'rgba(250,204,21,0.2)' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10 }}>
                  <span style={{ width: 7, height: 7, borderRadius: '50%', background: '#facc15', boxShadow: '0 0 8px #facc1588', flexShrink: 0 }} />
                  <span style={{ fontSize: 12, fontWeight: 600, color: '#facc15', letterSpacing: '0.05em' }}>RUNNING</span>
                </div>
                {activeJob && (
                  <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 4 }}>
                    Discovery: <span className="mono" style={{ color: 'var(--text-primary)' }}>{activeJob.country} {activeJob.year_from}–{activeJob.year_to}</span>
                    <br /><span className="mono" style={{ color: '#facc15' }}>page {activeJob.current_page}/{activeJob.total_pages ?? '?'}</span> · {activeJob.releases_saved} saved
                  </div>
                )}
                {checker?.active && (
                  <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
                    Platform checker: <span className="mono" style={{ color: '#60a5fa' }}>{checker.unchecked_count}</span> remaining
                  </div>
                )}
              </div>
            )}

            {/* Public domain note */}
            {data.public_domain > 0 && (
              <div className="card" style={{ borderColor: 'rgba(74,222,128,0.15)' }}>
                <div style={{ fontSize: 12, color: '#4ade80', fontWeight: 600, marginBottom: 4 }}>
                  {data.public_domain} PUBLIC DOMAIN
                </div>
                <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>
                  Released 70+ years ago, free to digitize and distribute
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
