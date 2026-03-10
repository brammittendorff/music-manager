import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api, type RipJob, type DriveInfo, type ReadyRelease } from '../api'
import { useState } from 'react'

const STATUS_CONFIG: Record<string, { color: string; dotClass: string; label: string }> = {
  detected:  { color: '#a78bfa', dotClass: 'detected', label: 'Detected' },
  ripping:   { color: '#F59E0B', dotClass: 'ripping',  label: 'Ripping' },
  splitting: { color: '#c084fc', dotClass: 'ripping',  label: 'Splitting' },
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

// ─── Start Rip Panel ─────────────────────────────────────────────────────────

function StartRipPanel() {
  const qc = useQueryClient()
  const [selectedDrive, setSelectedDrive] = useState<string | null>(null)
  const [selectedRelease, setSelectedRelease] = useState<ReadyRelease | null>(null)

  const { data: drives, isLoading: drivesLoading } = useQuery<DriveInfo[]>({
    queryKey: ['drives'],
    queryFn: api.drives,
    refetchInterval: 4_000,
  })

  const { data: readyReleases } = useQuery<ReadyRelease[]>({
    queryKey: ['ready-to-rip'],
    queryFn: api.readyToRip,
    refetchInterval: 10_000,
  })

  const startMutation = useMutation({
    mutationFn: () => {
      if (!selectedDrive || !selectedRelease) throw new Error('Select drive and release')
      return api.startRip(selectedDrive, selectedRelease.discogs_id, selectedRelease.watchlist_id)
    },
    onSuccess: () => {
      setSelectedRelease(null)
      qc.invalidateQueries({ queryKey: ['rip-jobs'] })
      qc.invalidateQueries({ queryKey: ['ready-to-rip'] })
    },
  })

  const drivesWithMedia = drives?.filter(d => d.has_media) ?? []
  const noDrives = drives && drives.length === 0
  const noMedia = drives && drives.length > 0 && drivesWithMedia.length === 0
  const noReleases = readyReleases && readyReleases.length === 0

  // Auto-select the only drive with media
  if (drivesWithMedia.length === 1 && !selectedDrive) {
    setSelectedDrive(drivesWithMedia[0].path)
  }

  return (
    <div style={{
      background: 'var(--bg-raised)',
      border: '1px solid var(--border)',
      borderRadius: 10,
      padding: 20,
      marginBottom: 20,
    }}>
      <div style={{ fontFamily: 'var(--font-display)', fontSize: 15, letterSpacing: '0.06em', marginBottom: 16, color: 'var(--gold)' }}>
        START NEW RIP
      </div>

      {/* Step 1: Drive detection */}
      <div style={{ marginBottom: 16 }}>
        <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 6, fontWeight: 600, letterSpacing: '0.05em' }}>
          1. CD/DVD DRIVE
        </div>
        {drivesLoading && <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>Scanning drives...</span>}
        {noDrives && (
          <div style={{ fontSize: 12, color: '#f87171', padding: '8px 12px', background: '#f8717112', borderRadius: 6, border: '1px solid #f8717130' }}>
            No CD/DVD drive detected. Connect an optical drive to continue.
          </div>
        )}
        {noMedia && (
          <div style={{ fontSize: 12, color: '#F59E0B', padding: '8px 12px', background: '#F59E0B12', borderRadius: 6, border: '1px solid #F59E0B30' }}>
            Drive found ({drives![0].path}) but no disc inserted. Insert a CD to continue.
          </div>
        )}
        {drivesWithMedia.length > 0 && (
          <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
            {drivesWithMedia.map(d => (
              <button
                key={d.path}
                onClick={() => setSelectedDrive(d.path)}
                style={{
                  padding: '6px 14px',
                  borderRadius: 6,
                  fontSize: 12,
                  fontWeight: 600,
                  border: `1px solid ${selectedDrive === d.path ? '#4ade80' : 'var(--border)'}`,
                  background: selectedDrive === d.path ? '#4ade8015' : 'var(--bg-hover)',
                  color: selectedDrive === d.path ? '#4ade80' : 'var(--text-secondary)',
                  cursor: 'pointer',
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                }}
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>
                </svg>
                {d.path}
                {d.label && <span style={{ color: 'var(--text-muted)', fontWeight: 400 }}>({d.label})</span>}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Step 2: Select release */}
      {selectedDrive && (
        <div style={{ marginBottom: 16 }}>
          <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 6, fontWeight: 600, letterSpacing: '0.05em' }}>
            2. SELECT RELEASE
          </div>
          {noReleases && (
            <div style={{ fontSize: 12, color: 'var(--text-muted)', padding: '8px 12px', background: 'var(--bg-hover)', borderRadius: 6 }}>
              No releases with status "ready to rip" in your watchlist.
              Mark a purchased release as "ready to rip" first.
            </div>
          )}
          {readyReleases && readyReleases.length > 0 && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 4, maxHeight: 220, overflowY: 'auto' }}>
              {readyReleases.map(r => {
                const isSelected = selectedRelease?.watchlist_id === r.watchlist_id
                return (
                  <button
                    key={r.watchlist_id}
                    onClick={() => setSelectedRelease(isSelected ? null : r)}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 10,
                      padding: '8px 12px',
                      borderRadius: 6,
                      border: `1px solid ${isSelected ? '#4ade80' : 'var(--border)'}`,
                      background: isSelected ? '#4ade8010' : 'var(--bg-hover)',
                      cursor: 'pointer',
                      textAlign: 'left',
                      width: '100%',
                    }}
                  >
                    {r.thumb_url && (
                      <img
                        src={r.thumb_url}
                        alt=""
                        style={{ width: 36, height: 36, borderRadius: 4, objectFit: 'cover', flexShrink: 0 }}
                      />
                    )}
                    {!r.thumb_url && (
                      <div style={{ width: 36, height: 36, borderRadius: 4, background: 'var(--bg-raised)', flexShrink: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" strokeWidth="1.5">
                          <circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>
                        </svg>
                      </div>
                    )}
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontSize: 13, color: isSelected ? '#4ade80' : 'var(--text-primary)', fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                        {r.artists[0] ?? 'Unknown'} — {r.title}
                      </div>
                      <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>
                        {r.year ?? '?'}{r.label ? ` · ${r.label}` : ''} · Discogs #{r.discogs_id}
                      </div>
                    </div>
                    {isSelected && (
                      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#4ade80" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }}>
                        <polyline points="20 6 9 17 4 12"/>
                      </svg>
                    )}
                  </button>
                )
              })}
            </div>
          )}
        </div>
      )}

      {/* Step 3: Start */}
      {selectedDrive && selectedRelease && (
        <div>
          <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 6, fontWeight: 600, letterSpacing: '0.05em' }}>
            3. RIP
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
            <button
              onClick={() => startMutation.mutate()}
              disabled={startMutation.isPending}
              style={{
                padding: '8px 20px',
                borderRadius: 6,
                fontSize: 13,
                fontWeight: 700,
                border: '1px solid #4ade80',
                background: '#4ade8020',
                color: '#4ade80',
                cursor: startMutation.isPending ? 'wait' : 'pointer',
                opacity: startMutation.isPending ? 0.6 : 1,
                letterSpacing: '0.04em',
              }}
            >
              {startMutation.isPending ? 'Starting...' : 'Start Rip'}
            </button>
            <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>
              {selectedRelease.artists[0]} — {selectedRelease.title} from {selectedDrive}
            </div>
          </div>
          {startMutation.isError && (
            <div style={{ fontSize: 12, color: '#f87171', marginTop: 8 }}>
              {(startMutation.error as Error).message}
            </div>
          )}
          {startMutation.isSuccess && (
            <div style={{ fontSize: 12, color: '#4ade80', marginTop: 8 }}>
              Rip job started! Scroll down to track progress.
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ─── Main Page ───────────────────────────────────────────────────────────────

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
        <h1 className="page-title">DIGITIZE</h1>
        <p className="page-subtitle">
          Rip CDs to MP3 320kbps with Discogs metadata and cover art
          {active.length > 0 && <span style={{ color: 'var(--gold)', marginLeft: 8 }}>● {active.length} active</span>}
        </p>
      </div>

      <StartRipPanel />

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">API error - is mm-api running?</div>}

      {data && data.length === 0 && (
        <div style={{ fontSize: 12, color: 'var(--text-muted)', textAlign: 'center', padding: 20 }}>
          No rip jobs yet. Select a drive and release above to start.
        </div>
      )}

      {data && data.length > 0 && (
        <>
          <div style={{ fontFamily: 'var(--font-display)', fontSize: 13, letterSpacing: '0.06em', color: 'var(--text-muted)', marginBottom: 10 }}>
            RIP HISTORY
          </div>
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
                        <span style={{ color: '#f87171', marginLeft: 8 }}>{job.error_msg}</span>
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
        </>
      )}
    </div>
  )
}
