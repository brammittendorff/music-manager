import { useQuery } from '@tanstack/react-query'
import { api, type RipJob } from '../api'

const STATUS_CONFIG: Record<string, { color: string; dotClass: string; label: string }> = {
  detected:  { color: '#a78bfa', dotClass: 'detected', label: 'Detected' },
  ripping:   { color: '#F59E0B', dotClass: 'ripping',  label: 'Ripping' },
  encoding:  { color: '#60a5fa', dotClass: 'encoding', label: 'Encoding' },
  tagging:   { color: '#34d399', dotClass: 'tagging',  label: 'Tagging' },
  done:      { color: '#4ade80', dotClass: 'done',     label: 'Done' },
  failed:    { color: '#f87171', dotClass: 'failed',   label: 'Failed' },
}

function formatTime(iso: string) {
  const d = new Date(iso)
  return d.toLocaleString('en-NL', {
    month: 'short', day: 'numeric',
    hour: '2-digit', minute: '2-digit',
  })
}

function duration(start: string, end: string | null) {
  if (!end) return 'in progress'
  const ms = new Date(end).getTime() - new Date(start).getTime()
  const m = Math.floor(ms / 60000)
  const s = Math.floor((ms % 60000) / 1000)
  return m > 0 ? `${m}m ${s}s` : `${s}s`
}

export default function RipJobs() {
  const { data, isLoading, error } = useQuery<RipJob[]>({
    queryKey: ['rip-jobs'],
    queryFn: api.ripJobs,
    refetchInterval: 5_000,
  })

  const active = data?.filter(j => !['done', 'failed'].includes(j.status)) ?? []

  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">RIP JOBS</h1>
        <p className="page-subtitle">
          CD ripping pipeline - auto-refreshes every 5 seconds
          {active.length > 0 && <span style={{ color: 'var(--gold)', marginLeft: 8 }}>● {active.length} active</span>}
        </p>
      </div>

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">⚠ API error - is mm-api running?</div>}

      {data && data.length === 0 && (
        <div className="empty-state">
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round">
            <path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>
          </svg>
          <span>No rip jobs yet.</span>
          <span style={{ fontSize: 12 }}>Insert a CD and run <code className="mono" style={{ background: 'var(--bg-hover)', padding: '2px 6px', borderRadius: 4 }}>mm rip daemon</code></span>
        </div>
      )}

      {data && data.length > 0 && (
        <div className="rip-list">
          {data.map(job => {
            const cfg = STATUS_CONFIG[job.status] ?? STATUS_CONFIG.detected
            return (
              <div key={job.id} className="rip-item">
                <div className={`rip-status-dot rip-status-dot--${cfg.dotClass}`} />
                <div className="rip-info">
                  <div className="rip-title">
                    {job.release_title ?? job.drive_path}
                    <span className="badge" style={{
                      marginLeft: 8,
                      background: `${cfg.color}18`,
                      color: cfg.color,
                      border: `1px solid ${cfg.color}30`,
                    }}>
                      {cfg.label}
                    </span>
                  </div>
                  <div className="rip-meta">
                    {job.drive_path} · {job.backend} · {job.track_count ?? '?'} tracks
                    {job.accuraterip_status && ` · AccurateRip: ${job.accuraterip_status}`}
                    {job.error_msg && (
                      <span style={{ color: '#f87171', marginLeft: 8 }}>⚠ {job.error_msg}</span>
                    )}
                  </div>
                  {job.output_dir && (
                    <div className="rip-meta" style={{ color: 'var(--text-muted)', fontSize: 10 }}>
                      {job.output_dir}
                    </div>
                  )}
                </div>
                <div className="rip-time">
                  <div>{formatTime(job.started_at)}</div>
                  <div style={{ color: 'var(--gold)', marginTop: 2 }}>
                    {duration(job.started_at, job.finished_at)}
                  </div>
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
