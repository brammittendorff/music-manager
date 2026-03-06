import { useState, useRef } from 'react'
import { createPortal } from 'react-dom'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api, type Release, type TrackCheck } from '../api'
import { X, ExternalLink, ShoppingCart, Plus } from 'lucide-react'

const PLATFORMS = ['spotify', 'deezer', 'apple_music', 'bandcamp', 'youtube_music']
const PLATFORM_LABELS: Record<string, string> = {
  spotify: 'SP', deezer: 'DZ', apple_music: 'AM', bandcamp: 'BC', youtube_music: 'YT',
}

function CopyrightBadge({ status }: { status: string }) {
  const label: Record<string, string> = {
    PUBLIC_DOMAIN: 'PD',
    LIKELY_PUBLIC_DOMAIN: '~PD',
    CHECK_REQUIRED: 'CHK',
    UNDER_COPYRIGHT: '©',
    UNKNOWN: '?',
  }
  return (
    <span className={`badge badge--${status}`}>
      {label[status] ?? status}
    </span>
  )
}

function PlatformBadges({ platforms }: { platforms: Release['platforms'] }) {
  return (
    <div className="platform-badges">
      {PLATFORMS.map(p => {
        const check = platforms.find(c => c.platform === p)
        const cls = !check ? 'unchecked' : check.found ? 'found' : 'missing'
        return (
          <span key={p} className={`badge badge--${cls}`} title={p}>
            {PLATFORM_LABELS[p]}
          </span>
        )
      })}
    </div>
  )
}

function TrackResults({ releaseId }: { releaseId: string }) {
  const { data: tracks, isLoading } = useQuery<TrackCheck[]>({
    queryKey: ['release-tracks', releaseId],
    queryFn: () => api.releaseTracks(releaseId),
  })

  if (isLoading) return <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>Loading tracks…</div>
  if (!tracks || tracks.length === 0) return (
    <div style={{ fontSize: 12, color: 'var(--text-muted)', fontStyle: 'italic' }}>
      No track-level results yet - platform checker will populate these.
    </div>
  )

  // Group by track title preserving order
  const trackMap = new Map<string, Map<string, TrackCheck>>()
  for (const tc of tracks) {
    if (!trackMap.has(tc.track_title)) trackMap.set(tc.track_title, new Map())
    trackMap.get(tc.track_title)!.set(tc.platform, tc)
  }
  const platforms = [...new Set(tracks.map(t => t.platform))].sort()

  return (
    <div>
      <div className="meta-key" style={{ marginBottom: 8 }}>Tracks</div>
      <div style={{ overflowX: 'auto' }}>
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 11 }}>
          <thead>
            <tr>
              <th style={{ textAlign: 'left', padding: '4px 6px', color: 'var(--text-muted)', fontWeight: 500 }}>Track</th>
              {platforms.map(p => (
                <th key={p} style={{ textAlign: 'center', padding: '4px 6px', color: 'var(--text-muted)', fontWeight: 500, whiteSpace: 'nowrap' }}>
                  {PLATFORM_LABELS[p] ?? p}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {[...trackMap.entries()].map(([title, byPlatform]) => (
              <tr key={title} style={{ borderTop: '1px solid var(--border)' }}>
                <td style={{ padding: '5px 6px', color: 'var(--text-primary)', maxWidth: 160, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                  title={title}>{title}</td>
                {platforms.map(p => {
                  const tc = byPlatform.get(p)
                  return (
                    <td key={p} style={{ textAlign: 'center', padding: '5px 6px' }}>
                      {!tc ? (
                        <span style={{ color: 'var(--text-muted)' }}>—</span>
                      ) : tc.found ? (
                        <a href={tc.platform_url ?? '#'} target="_blank" rel="noreferrer"
                          style={{ color: '#4ade80', textDecoration: 'none', fontSize: 14 }} title="Found">✓</a>
                      ) : (
                        <span style={{ color: '#f87171', fontSize: 14 }} title="Not found">✗</span>
                      )}
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

function DetailPanel({ release, onClose }: { release: Release; onClose: () => void }) {
  const qc = useQueryClient()
  const addMut = useMutation({
    mutationFn: () => api.addToWatchlist(release.discogs_id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['releases'] }),
  })

  return createPortal(
    <div className="detail-overlay" onClick={onClose}>
      <div className="detail-panel" onClick={e => e.stopPropagation()}>
        <button className="detail-close" onClick={onClose}><X size={14} /></button>

        <div>
          <div style={{ fontSize: 11, letterSpacing: '0.1em', textTransform: 'uppercase', color: 'var(--gold)', marginBottom: 6 }}>
            {release.artists.join(', ')}
          </div>
          <div className="detail-title">{release.title}</div>
        </div>

        <div className="detail-meta">
          <div className="meta-item">
            <span className="meta-key">Year</span>
            <span className="meta-val mono">{release.year ?? '—'}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Label</span>
            <span className="meta-val">{release.label ?? '—'}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Country</span>
            <span className="meta-val">{release.country_code}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Formats</span>
            <span className="meta-val">{release.formats.join(', ') || '—'}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Copyright</span>
            <span className="meta-val"><CopyrightBadge status={release.copyright_status} /></span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Genres</span>
            <span className="meta-val" style={{ fontSize: 12 }}>{release.genres.join(', ') || '—'}</span>
          </div>
        </div>

        <div>
          <div className="meta-key" style={{ marginBottom: 8 }}>Platform availability</div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
            {PLATFORMS.map(p => {
              const check = release.platforms.find(c => c.platform === p)
              const cls = !check ? 'unchecked' : check.found ? 'found' : 'missing'
              return (
                <a key={p}
                  href={check?.platform_url ?? '#'}
                  target={check?.platform_url ? '_blank' : undefined}
                  rel="noreferrer"
                  className={`badge badge--${cls}`}
                  style={{ gap: 5 }}
                >
                  {PLATFORM_LABELS[p]} {p.replace('_', ' ')}
                  {check?.platform_url && <ExternalLink size={10} />}
                </a>
              )
            })}
          </div>
        </div>

        <TrackResults releaseId={release.id} />

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <a href={release.buy_url} target="_blank" rel="noreferrer"
            className="detail-action-btn" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8 }}>
            <ShoppingCart size={14} /> Buy on Discogs
          </a>
          {!release.in_watchlist ? (
            <button className="detail-action-btn detail-action-btn--secondary"
              onClick={() => addMut.mutate()}
              disabled={addMut.isPending}>
              <Plus size={14} /> {addMut.isPending ? 'Adding…' : 'Add to watchlist'}
            </button>
          ) : (
            <div style={{ textAlign: 'center', fontSize: 12, color: 'var(--gold)' }}>
              ✓ On watchlist ({release.watchlist_status})
            </div>
          )}
          <a href={release.discogs_url} target="_blank" rel="noreferrer"
            className="detail-action-btn detail-action-btn--secondary"
            style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8 }}>
            <ExternalLink size={14} /> View on Discogs
          </a>
        </div>
      </div>
    </div>,
    document.body
  )
}

export default function Releases() {
  const [missingOnly, setMissingOnly] = useState(false)
  const [platformStatus, setPlatformStatus] = useState('')
  const [selectedPlatforms, setSelectedPlatforms] = useState<Set<string>>(new Set())
  const [selected, setSelected] = useState<Release | null>(null)
  const [page, setPage] = useState(0)
  const LIMIT = 100
  const qc = useQueryClient()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [importing, setImporting] = useState(false)

  const togglePlatform = (p: string) => {
    setSelectedPlatforms(prev => {
      const next = new Set(prev)
      if (next.has(p)) next.delete(p)
      else next.add(p)
      return next
    })
    setPage(0)
  }

  const platformsParam = selectedPlatforms.size > 0 ? [...selectedPlatforms].join(',') : undefined

  const { data, isLoading, error } = useQuery({
    queryKey: ['releases', missingOnly, platformStatus, platformsParam, page],
    queryFn: () => api.releases({
      missing_only: missingOnly,
      platform_status: platformStatus || undefined,
      platforms: platformsParam,
      limit: LIMIT, offset: page * LIMIT,
    }),
  })

  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">RELEASES</h1>
        <p className="page-subtitle">
          {data ? `${data.total.toLocaleString()} releases` : 'Browse Discogs discoveries'}
        </p>
      </div>

      <div className="filter-bar" style={{ flexWrap: 'wrap', gap: 6 }}>
        {/* Missing only toggle */}
        {/* Status filter - mutually exclusive */}
        {([
          { value: 'unchecked', label: 'Unchecked',        title: 'Not yet checked on any platform',          color: '#94a3b8' },
          { value: 'missing',   label: 'Not on platforms', title: 'Checked - missing from all platforms',     color: '#f87171' },
          { value: 'found',     label: 'On platforms',     title: 'Found on at least one streaming platform', color: '#4ade80' },
        ] as const).map(({ value, label, title, color }) => {
          const active = platformStatus === value
          return (
            <button key={value}
              onClick={() => { setPlatformStatus(active ? '' : value); setMissingOnly(false); setPage(0) }}
              title={title}
              style={{
                padding: '5px 12px', borderRadius: 6, fontSize: 12, fontWeight: 600,
                border: `1px solid ${active ? color : 'var(--border)'}`,
                background: active ? color + '22' : 'transparent',
                color: active ? color : 'var(--text-muted)',
                cursor: 'pointer', transition: 'all 0.15s',
              }}>
              {label}
            </button>
          )
        })}

        {/* Divider */}
        <span style={{ width: 1, background: 'var(--border)', alignSelf: 'stretch', margin: '0 2px' }} />

        {/* Per-platform missing toggles - can combine */}
        <span style={{ fontSize: 10, color: 'var(--text-muted)', alignSelf: 'center', letterSpacing: '0.08em' }}>MISSING ON</span>
        {PLATFORMS.map(p => {
          const active = selectedPlatforms.has(p)
          return (
            <button key={p}
              onClick={() => togglePlatform(p)}
              title={`${active ? 'Remove: ' : 'Missing on '}${p.replace(/_/g, ' ')}`}
              style={{
                padding: '4px 9px', borderRadius: 4, fontSize: 11, fontWeight: 700,
                border: `1px solid ${active ? '#f87171' : 'var(--border)'}`,
                background: active ? '#f8717122' : 'transparent',
                color: active ? '#f87171' : 'var(--text-muted)',
                letterSpacing: '0.04em', cursor: 'pointer', transition: 'all 0.15s',
              }}>
              {PLATFORM_LABELS[p]}
            </button>
          )
        })}
      </div>

      <div style={{ display: 'flex', gap: 8, marginBottom: 12, justifyContent: 'flex-end' }}>
        <button
          className="toggle-btn"
          onClick={() => api.exportReleases().catch(console.error)}
          title="Download all releases as JSON"
        >
          ↓ Backup
        </button>

        <button
          className="toggle-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={importing}
          title="Import releases from JSON file"
        >
          {importing ? 'Importing…' : '↑ Import'}
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept=".json"
          style={{ display: 'none' }}
          onChange={async e => {
            const file = e.target.files?.[0]
            if (!file) return
            setImporting(true)
            try {
              const result = await api.importReleases(file)
              alert(`Imported ${result.imported} releases`)
              qc.invalidateQueries({ queryKey: ['releases'] })
              qc.invalidateQueries({ queryKey: ['stats'] })
            } catch (err) {
              alert('Import failed: ' + err)
            } finally {
              setImporting(false)
              e.target.value = ''
            }
          }}
        />

        <button
          className="toggle-btn"
          style={{ borderColor: '#f87171', color: '#f87171' }}
          onClick={async () => {
            if (!window.confirm('Delete ALL releases, platform checks and watchlist entries?')) return
            await api.clearReleases()
            qc.invalidateQueries({ queryKey: ['releases'] })
            qc.invalidateQueries({ queryKey: ['stats'] })
          }}
          title="Delete all releases"
        >
          ✕ Clear
        </button>
      </div>

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">⚠ API error - is mm-api running on port 3001?</div>}

      {data && (
        <>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Artist</th>
                  <th>Title</th>
                  <th>Year</th>
                  <th>Label</th>
                  <th>Copyright</th>
                  <th>Platforms</th>
                  <th>Format</th>
                </tr>
              </thead>
              <tbody>
                {data.data.map(r => (
                  <tr key={r.id} onClick={() => setSelected(r)}>
                    <td className="td-artist">{r.artists[0] ?? '—'}</td>
                    <td className="td-title">{r.title}</td>
                    <td className="td-year">{r.year ?? '—'}</td>
                    <td style={{ color: 'var(--text-muted)', fontSize: 12, maxWidth: 120, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {r.label ?? '—'}
                    </td>
                    <td><CopyrightBadge status={r.copyright_status} /></td>
                    <td><PlatformBadges platforms={r.platforms} /></td>
                    <td style={{ fontSize: 11, color: 'var(--text-muted)' }}>{r.formats.slice(0,2).join(' · ') || '—'}</td>
                  </tr>
                ))}
                {data.data.length === 0 && (
                  <tr><td colSpan={7}>
                    <div className="empty-state">No releases found for these filters.</div>
                  </td></tr>
                )}
              </tbody>
            </table>
          </div>

          {data.total > LIMIT && (
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 14, justifyContent: 'flex-end' }}>
              <button className="toggle-btn" disabled={page === 0} onClick={() => setPage(p => p - 1)}>← Prev</button>
              <span className="mono" style={{ fontSize: 12, color: 'var(--text-muted)' }}>
                {page * LIMIT + 1}–{Math.min((page + 1) * LIMIT, data.total)} / {data.total}
              </span>
              <button className="toggle-btn" disabled={(page + 1) * LIMIT >= data.total} onClick={() => setPage(p => p + 1)}>Next →</button>
            </div>
          )}
        </>
      )}

      {selected && <DetailPanel release={selected} onClose={() => setSelected(null)} />}
    </div>
  )
}
