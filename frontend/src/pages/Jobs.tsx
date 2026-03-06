import { useState } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api, type DiscoveryJob, type StartJobRequest } from '../api'

function PlatformCheckerPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['platform-checker'],
    queryFn: api.platformCheckerStatus,
    refetchInterval: (q) => q.state.data?.active ? 3000 : 10000,
  })
  const start = useMutation({ mutationFn: api.platformCheckerStart, onSuccess: () => qc.invalidateQueries({ queryKey: ['platform-checker'] }) })
  const stop  = useMutation({ mutationFn: api.platformCheckerStop,  onSuccess: () => qc.invalidateQueries({ queryKey: ['platform-checker'] }) })

  return (
    <div className="card" style={{ marginBottom: 14 }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 12 }}>
        <div style={{ fontFamily: 'var(--font-display)', fontSize: 18, letterSpacing: '0.06em', color: 'var(--gold)' }}>PLATFORM CHECKER</div>
        <span style={{ fontSize: 11, fontWeight: 600, padding: '3px 10px', borderRadius: 99,
          background: data?.active ? '#facc1522' : '#64748b22',
          color: data?.active ? '#facc15' : '#64748b',
          border: `1px solid ${data?.active ? '#facc1544' : '#64748b44'}` }}>
          {data?.active ? 'RUNNING' : 'IDLE'}
        </span>
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8, marginBottom: 12 }}>
        <div style={{ background: 'var(--bg-raised)', borderRadius: 6, padding: '8px 10px' }}>
          <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 2 }}>UNCHECKED</div>
          <div className="mono" style={{ fontSize: 16 }}>{data?.unchecked_count ?? '—'}</div>
        </div>
        <div style={{ background: 'var(--bg-raised)', borderRadius: 6, padding: '8px 10px' }}>
          <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 2 }}>CHECKED</div>
          <div className="mono" style={{ fontSize: 16 }}>{data?.total_checked ?? '—'}</div>
        </div>
      </div>

      {/* Active / skipped platforms */}
      {data && (
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginBottom: 12 }}>
          {(data.active_platforms ?? []).map((p: string) => (
            <span key={p} style={{
              fontSize: 11, fontWeight: 600, padding: '2px 8px', borderRadius: 4,
              background: '#4ade8022', color: '#4ade80', border: '1px solid #4ade8044',
            }}>{p.replace('_', ' ')}</span>
          ))}
          {(data.skipped_platforms ?? []).map((p: string) => (
            <span key={p} title="Not configured - add API key to .env" style={{
              fontSize: 11, fontWeight: 600, padding: '2px 8px', borderRadius: 4,
              background: '#f8717111', color: '#f87171', border: '1px solid #f8717133',
              textDecoration: 'line-through', opacity: 0.7,
            }}>{p.replace('_', ' ')}</span>
          ))}
        </div>
      )}
      <div style={{ display: 'flex', gap: 8 }}>
        <button onClick={() => start.mutate()} disabled={data?.active || start.isPending}
          style={{ padding: '8px 18px', borderRadius: 6, border: '1px solid var(--gold)',
            background: data?.active ? 'var(--bg-raised)' : 'var(--gold)',
            color: data?.active ? 'var(--text-muted)' : '#000',
            fontWeight: 600, fontSize: 13, opacity: data?.active ? 0.5 : 1, cursor: data?.active ? 'not-allowed' : 'pointer' }}>
          Start
        </button>
        <button onClick={() => stop.mutate()} disabled={!data?.active || stop.isPending}
          style={{ padding: '8px 18px', borderRadius: 6, border: '1px solid var(--border)',
            background: 'transparent', color: data?.active ? 'var(--text-primary)' : 'var(--text-muted)',
            fontWeight: 600, fontSize: 13, opacity: data?.active ? 1 : 0.4, cursor: data?.active ? 'pointer' : 'not-allowed' }}>
          Stop
        </button>
      </div>
    </div>
  )
}

const STATUS_COLOR: Record<string, string> = {
  running:   '#facc15',
  paused:    '#fb923c',
  completed: '#4ade80',
  failed:    '#f87171',
  cancelled: '#94a3b8',
}

function StatusBadge({ status }: { status: string }) {
  const color = STATUS_COLOR[status] ?? '#64748b'
  return (
    <span style={{
      fontSize: 10, fontWeight: 700, letterSpacing: '0.08em',
      padding: '2px 8px', borderRadius: 99,
      background: color + '22', color, border: `1px solid ${color}44`,
    }}>
      {status.toUpperCase()}
    </span>
  )
}

function JobRow({ job }: { job: DiscoveryJob }) {
  const qc = useQueryClient()

  const resume = useMutation({
    mutationFn: () => api.discoveryResume(job.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['discoveryJobs'] }),
  })
  const pause = useMutation({
    mutationFn: () => api.discoveryPause(job.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['discoveryJobs'] }),
  })
  const del = useMutation({
    mutationFn: () => api.discoveryDeleteJob(job.id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['discoveryJobs'] }),
  })

  const isRunning = job.status === 'running'
  const canResume = job.status === 'paused' || job.status === 'failed' || job.status === 'cancelled'
  const canDelete = !isRunning
  const pct = job.total_pages && job.current_page
    ? Math.round((job.current_page / job.total_pages) * 100)
    : 0

  return (
    <div style={{
      background: 'var(--bg-raised)', borderRadius: 8, padding: '12px 14px',
      border: isRunning ? '1px solid var(--gold)44' : '1px solid var(--border)',
    }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 8 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <StatusBadge status={job.status} />
          <span className="mono" style={{ fontSize: 12, color: 'var(--text-primary)' }}>
            {job.country_code} · {job.year_from}–{job.year_to} · {job.format_filter}
          </span>
        </div>
        <div style={{ display: 'flex', gap: 6 }}>
          {canResume && (
            <button
              onClick={() => resume.mutate()}
              disabled={resume.isPending}
              style={{
                padding: '4px 12px', borderRadius: 5, border: '1px solid var(--gold)',
                background: 'transparent', color: 'var(--gold)',
                fontSize: 12, fontWeight: 600, cursor: 'pointer',
              }}
            >
              {resume.isPending ? '…' : 'Resume'}
            </button>
          )}
          {isRunning && (
            <button
              onClick={() => pause.mutate()}
              disabled={pause.isPending}
              style={{
                padding: '4px 12px', borderRadius: 5, border: '1px solid var(--border)',
                background: 'transparent', color: 'var(--text-primary)',
                fontSize: 12, fontWeight: 600, cursor: 'pointer',
              }}
            >
              {pause.isPending ? '…' : 'Pause'}
            </button>
          )}
          {canDelete && (
            <button
              onClick={() => {
                if (window.confirm('Delete this discovery job?')) del.mutate()
              }}
              disabled={del.isPending}
              style={{
                padding: '4px 12px', borderRadius: 5, border: '1px solid #f8717144',
                background: 'transparent', color: '#f87171',
                fontSize: 12, fontWeight: 600, cursor: 'pointer',
              }}
            >
              {del.isPending ? '…' : 'Delete'}
            </button>
          )}
        </div>
      </div>

      {/* Progress bar */}
      <div style={{ background: 'var(--bg-hover)', borderRadius: 3, height: 4, marginBottom: 8, overflow: 'hidden' }}>
        <div style={{
          height: '100%', borderRadius: 3,
          width: `${pct}%`,
          background: isRunning ? 'var(--gold)' : (STATUS_COLOR[job.status] ?? '#64748b'),
          transition: 'width 0.5s ease',
        }} />
      </div>

      <div style={{ display: 'flex', gap: 16, fontSize: 12, color: 'var(--text-muted)' }}>
        <span>Page <span style={{ color: 'var(--text-primary)' }}>{job.current_page}</span>{job.total_pages ? ` / ${job.total_pages}` : ''}</span>
        <span>Saved <span style={{ color: 'var(--text-primary)' }}>{job.releases_saved.toLocaleString()}</span></span>
        <span>Missing <span style={{ color: 'var(--text-primary)' }}>{job.missing_count.toLocaleString()}</span></span>
        {job.genres.length > 0 && (
          <span style={{ marginLeft: 'auto' }}>{job.genres.slice(0, 3).join(', ')}{job.genres.length > 3 ? '…' : ''}</span>
        )}
      </div>

      {job.error_msg && job.error_msg !== 'API restarted' && (
        <div style={{ fontSize: 11, color: '#f87171', marginTop: 6, padding: '4px 8px', background: '#f8717111', borderRadius: 4 }}>
          {job.error_msg}
        </div>
      )}
      {job.status === 'paused' && job.error_msg === 'API restarted' && (
        <div style={{ fontSize: 11, color: '#94a3b8', marginTop: 6, padding: '4px 8px', background: '#94a3b811', borderRadius: 4 }}>
          Paused after API restart - click Resume to continue
        </div>
      )}
    </div>
  )
}

function DiscoveryJobs() {
  const qc = useQueryClient()

  const [form, setForm] = useState<StartJobRequest>({
    country: 'Netherlands',
    country_code: 'NL',
    genres: [],
    year_from: 1950,
    year_to: 2005,
    format_filter: 'Album',
    max_pages: undefined,
  })

  const { data: jobs = [] } = useQuery<DiscoveryJob[]>({
    queryKey: ['discoveryJobs'],
    queryFn: api.discoveryJobs,
    refetchInterval: (query) => {
      const data = query.state.data
      return Array.isArray(data) && data.some(j => j.status === 'running') ? 3_000 : 10_000
    },
  })

  const create = useMutation({
    mutationFn: () => api.discoveryCreate(form),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['discoveryJobs'] }),
  })

  const anyRunning = jobs.some(j => j.status === 'running')

  const inputStyle = {
    background: 'var(--bg-hover)', border: '1px solid var(--border)',
    borderRadius: 5, color: 'var(--text-primary)', fontSize: 12, padding: '5px 8px',
  }

  return (
    <div className="card">
      <div style={{ fontFamily: 'var(--font-display)', fontSize: 18, letterSpacing: '0.06em', color: 'var(--gold)', marginBottom: 14 }}>
        DISCOVERY JOBS
      </div>

      {/* New job form */}
      <div style={{ background: 'var(--bg-hover)', borderRadius: 8, padding: 12, marginBottom: 16 }}>
        <div style={{ fontSize: 11, color: 'var(--text-muted)', letterSpacing: '0.06em', marginBottom: 10 }}>NEW JOB</div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 8, marginBottom: 10 }}>
          <div>
            <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 3 }}>Country</div>
            <input
              style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              value={form.country ?? ''}
              onChange={e => setForm(f => ({ ...f, country: e.target.value }))}
              placeholder="Netherlands"
            />
          </div>
          <div>
            <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 3 }}>Code</div>
            <input
              style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              value={form.country_code ?? ''}
              onChange={e => setForm(f => ({ ...f, country_code: e.target.value }))}
              placeholder="NL"
            />
          </div>
          <div>
            <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 3 }}>Format</div>
            <select
              style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              value={form.format_filter ?? 'Album'}
              onChange={e => setForm(f => ({ ...f, format_filter: e.target.value }))}
            >
              <option value="Album">Album</option>
              <option value="Single">Single</option>
              <option value="EP">EP</option>
              <option value="Compilation">Compilation</option>
            </select>
          </div>
          <div>
            <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 3 }}>Year from</div>
            <input
              type="number"
              style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              value={form.year_from ?? 1950}
              onChange={e => setForm(f => ({ ...f, year_from: Number(e.target.value) }))}
            />
          </div>
          <div>
            <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 3 }}>Year to</div>
            <input
              type="number"
              style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              value={form.year_to ?? 2005}
              onChange={e => setForm(f => ({ ...f, year_to: Number(e.target.value) }))}
            />
          </div>
          <div>
            <div style={{ fontSize: 10, color: 'var(--text-muted)', marginBottom: 3 }}>Max pages</div>
            <input
              type="number"
              style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              value={form.max_pages ?? ''}
              onChange={e => setForm(f => ({ ...f, max_pages: e.target.value ? Number(e.target.value) : undefined }))}
              placeholder="default"
            />
          </div>
        </div>
        <button
          onClick={() => create.mutate()}
          disabled={anyRunning || create.isPending}
          style={{
            padding: '7px 20px', borderRadius: 6,
            border: '1px solid var(--gold)',
            background: anyRunning ? 'var(--bg-raised)' : 'var(--gold)',
            color: anyRunning ? 'var(--text-muted)' : '#000',
            fontWeight: 700, fontSize: 13,
            cursor: anyRunning ? 'not-allowed' : 'pointer',
            opacity: anyRunning ? 0.5 : 1,
          }}
        >
          {create.isPending ? 'Starting…' : anyRunning ? 'Job running…' : 'Start Job'}
        </button>
        {anyRunning && (
          <span style={{ marginLeft: 10, fontSize: 12, color: 'var(--text-muted)' }}>
            Pause the running job before starting a new one
          </span>
        )}
      </div>

      {/* Jobs list */}
      {jobs.length === 0 ? (
        <p style={{ fontSize: 13, color: 'var(--text-muted)', textAlign: 'center', padding: '20px 0' }}>
          No discovery jobs yet. Configure filters above and click Start Job.
        </p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          {jobs.map(job => <JobRow key={job.id} job={job} />)}
        </div>
      )}
    </div>
  )
}

export default function Jobs() {
  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">JOBS</h1>
        <p className="page-subtitle">Discovery job queue - scan Discogs for missing Dutch music</p>
      </div>
      <PlatformCheckerPanel />
      <DiscoveryJobs />
    </div>
  )
}
