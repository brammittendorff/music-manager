import { useState, useRef } from 'react'
import { createPortal } from 'react-dom'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api, type Release, type TrackCheck } from '../api'
import { X, ExternalLink, ShoppingCart, Plus } from 'lucide-react'

// Persist filters across route changes (survives component remount)
const savedFilters = {
  platformStatus: '',
  selectedPlatforms: new Set<string>(),
  page: 0,
  sortBy: '',
  sortDir: 'asc' as 'asc' | 'desc',
}

const PLATFORMS = ['spotify', 'deezer', 'apple_music', 'bandcamp', 'youtube_music']
const PLATFORM_LABELS: Record<string, string> = {
  spotify: 'SP', deezer: 'DZ', apple_music: 'AM', bandcamp: 'BC', youtube_music: 'YT',
}

function PopularityDot({ score }: { score: number | null }) {
  if (score == null) return null
  const color = score > 0.7 ? '#f87171' : score > 0.4 ? '#fb923c' : '#64748b'
  const label = score > 0.7 ? 'hot' : score > 0.4 ? 'warm' : 'cool'
  const pct = Math.round(score * 100)
  return (
    <span title={`Popularity: ${pct}% (${label})`} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
      <span style={{
        display: 'inline-block',
        width: 28, height: 6, borderRadius: 3,
        background: `linear-gradient(90deg, ${color} ${pct}%, #1e293b ${pct}%)`,
        border: '1px solid #334155',
      }} />
      <span style={{ fontSize: 10, fontFamily: 'var(--font-mono)', color }}>{pct}</span>
    </span>
  )
}

function PriceTag({ release }: { release: Release }) {
  if (release.lowest_price_eur == null) return null
  const price = Number(release.lowest_price_eur)
  if (isNaN(price)) return null
  return (
    <span style={{
      fontSize: 10, fontFamily: 'var(--font-mono)',
      color: price <= 10 ? '#4ade80' : price <= 25 ? '#facc15' : '#f87171',
    }}>
      {'\u20AC'}{Math.round(price)}
      {release.num_for_sale != null && (
        <span style={{ color: 'var(--text-muted)', fontWeight: 400 }}> ({release.num_for_sale})</span>
      )}
    </span>
  )
}

function SortArrow({ column, activeCol, activeDir }: { column: string; activeCol: string; activeDir: string }) {
  if (column !== activeCol) return <span style={{ opacity: 0.2, marginLeft: 4, fontSize: 10 }}>↕</span>
  return <span style={{ marginLeft: 4, fontSize: 10 }}>{activeDir === 'asc' ? '↑' : '↓'}</span>
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
        const cls = !check ? 'unchecked' : check.error ? 'error' : check.found ? 'found' : 'missing'
        return (
          <span key={p} className={`badge badge--${cls}`} title={check?.error ? `${p} (error)` : p}>
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
                        <span style={{ color: 'var(--text-muted)' }}>/</span>
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

function DetailPanel({ releaseId, onClose }: { releaseId: string; onClose: () => void }) {
  const qc = useQueryClient()
  const { data: release } = useQuery<Release>({
    queryKey: ['release-detail', releaseId],
    queryFn: () => api.release(releaseId),
  })
  const addMut = useMutation({
    mutationFn: () => api.addToWatchlist(release!.discogs_id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['releases'] })
      qc.invalidateQueries({ queryKey: ['release-detail', releaseId] })
    },
  })

  if (!release) return null

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
            <span className="meta-val mono">{release.year ?? ''}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Label</span>
            <span className="meta-val">{release.label ?? ''}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Country</span>
            <span className="meta-val">{release.country_code}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Formats</span>
            <span className="meta-val">{release.formats.join(', ') || ''}</span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Copyright</span>
            <span className="meta-val"><CopyrightBadge status={release.copyright_status} /></span>
          </div>
          <div className="meta-item">
            <span className="meta-key">Genres</span>
            <span className="meta-val" style={{ fontSize: 12 }}>{release.genres.join(', ') || ''}</span>
          </div>
          {release.lowest_price_eur != null && (
            <div className="meta-item">
              <span className="meta-key">Price</span>
              <span className="meta-val"><PriceTag release={release} /></span>
            </div>
          )}
          {release.popularity_score != null && (
            <div className="meta-item">
              <span className="meta-key">Popularity</span>
              <span className="meta-val"><PopularityDot score={release.popularity_score} /></span>
            </div>
          )}
          {(release.discogs_want != null || release.discogs_have != null) && (
            <div className="meta-item">
              <span className="meta-key">Discogs</span>
              <span className="meta-val mono" style={{ fontSize: 12 }}>
                {release.discogs_want ?? 0} want / {release.discogs_have ?? 0} have
                {release.discogs_rating != null && release.discogs_rating_count != null && release.discogs_rating_count >= 3 && (
                  <span style={{ marginLeft: 8, color: '#facc15' }}>
                    {release.discogs_rating.toFixed(1)}/5 ({release.discogs_rating_count})
                  </span>
                )}
              </span>
            </div>
          )}
          {release.lastfm_playcount != null && release.lastfm_playcount > 0 && (
            <div className="meta-item">
              <span className="meta-key">Last.fm</span>
              <span className="meta-val mono" style={{ fontSize: 12 }}>
                {release.lastfm_playcount.toLocaleString()} plays / {(release.lastfm_listeners ?? 0).toLocaleString()} listeners
              </span>
            </div>
          )}
          {release.has_wikipedia && (
            <div className="meta-item">
              <span className="meta-key">Wikipedia</span>
              <span className="meta-val" style={{ fontSize: 12, color: '#4ade80' }}>Article exists</span>
            </div>
          )}
        </div>

        <div>
          <div className="meta-key" style={{ marginBottom: 8 }}>Platform availability</div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
            {PLATFORMS.map(p => {
              const check = release.platforms.find(c => c.platform === p)
              const cls = !check ? 'unchecked' : check.error ? 'error' : check.found ? 'found' : 'missing'
              return (
                <a key={p}
                  href={check?.platform_url ?? '#'}
                  target={check?.platform_url ? '_blank' : undefined}
                  rel="noreferrer"
                  className={`badge badge--${cls}`}
                  style={{ gap: 5 }}
                  title={check?.error ? `${p} returned an error` : undefined}
                >
                  {PLATFORM_LABELS[p]} {p.replace('_', ' ')}
                  {check?.error && ' ⚠'}
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
            <div style={{
              textAlign: 'center', fontSize: 12, fontWeight: 600,
              padding: '8px 12px', borderRadius: 6,
              background: 'rgba(250,204,21,0.08)', border: '1px solid rgba(250,204,21,0.25)',
              color: 'var(--gold)',
            }}>
              On watchlist ({release.watchlist_status})
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
  const [platformStatus, setPlatformStatusRaw] = useState(savedFilters.platformStatus)
  const [selectedPlatforms, setSelectedPlatformsRaw] = useState(savedFilters.selectedPlatforms)
  const [page, setPageRaw] = useState(savedFilters.page)
  const [selected, setSelected] = useState<string | null>(null)
  const [sortBy, setSortByRaw] = useState(savedFilters.sortBy)
  const [sortDir, setSortDirRaw] = useState(savedFilters.sortDir)
  const LIMIT = 100
  const qc = useQueryClient()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [importing, setImporting] = useState(false)

  const setPlatformStatus = (v: string) => {
    savedFilters.platformStatus = v
    savedFilters.page = 0
    setPlatformStatusRaw(v)
    setPageRaw(0)
  }

  const setPage = (p: number | ((prev: number) => number)) => {
    const next = typeof p === 'function' ? p(page) : p
    savedFilters.page = next
    setPageRaw(next)
  }

  const togglePlatform = (p: string) => {
    setSelectedPlatformsRaw(prev => {
      const next = new Set(prev)
      if (next.has(p)) next.delete(p)
      else next.add(p)
      savedFilters.selectedPlatforms = next
      savedFilters.page = 0
      setPageRaw(0)
      return next
    })
  }

  const toggleSort = (column: string, defaultDir: 'asc' | 'desc' = 'asc') => {
    if (sortBy === column) {
      // Same column: toggle direction
      const newDir = sortDir === 'asc' ? 'desc' : 'asc'
      savedFilters.sortDir = newDir
      setSortDirRaw(newDir)
    } else {
      // New column
      savedFilters.sortBy = column
      savedFilters.sortDir = defaultDir
      setSortByRaw(column)
      setSortDirRaw(defaultDir)
    }
    savedFilters.page = 0
    setPageRaw(0)
  }

  const platformsParam = selectedPlatforms.size > 0 ? [...selectedPlatforms].join(',') : undefined

  const sortParam = sortBy ? `${sortBy}_${sortDir}` : undefined

  const { data, isLoading, error } = useQuery({
    queryKey: ['releases', platformStatus, platformsParam, page, sortBy, sortDir],
    queryFn: () => api.releases({
      platform_status: platformStatus || undefined,
      platforms: platformsParam,
      sort_by: sortParam,
      limit: LIMIT, offset: page * LIMIT,
    }),
    refetchInterval: 15_000,
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
              onClick={() => setPlatformStatus(active ? '' : value)}
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
          onClick={async () => {
            try {
              const result = await api.enrichReleases()
              alert(result.message + ` (${result.releases_to_enrich} releases)`)
            } catch (err) {
              alert('Enrich failed: ' + err)
            }
          }}
          title="Fetch popularity data from Discogs, Last.fm and Wikipedia"
        >
          Enrich
        </button>

        <button
          className="toggle-btn"
          onClick={() => api.exportReleases().catch(console.error)}
          title="Download all releases as JSON"
        >
          Backup
        </button>

        <button
          className="toggle-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={importing}
          title="Import releases from JSON file"
        >
          {importing ? 'Importing…' : 'Import'}
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
            try {
              await api.clearReleases()
              qc.invalidateQueries({ queryKey: ['releases'] })
              qc.invalidateQueries({ queryKey: ['stats'] })
            } catch (err) {
              alert('Cannot clear: ' + err)
            }
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
                  {([
                    { key: 'artist', label: 'Artist', dir: 'asc' as const },
                    { key: 'title', label: 'Title', dir: 'asc' as const },
                    { key: 'year', label: 'Year', dir: 'desc' as const },
                    { key: 'label', label: 'Label', dir: 'asc' as const },
                    { key: 'copyright', label: 'Copyright', dir: 'asc' as const },
                    { key: '', label: 'Platforms', dir: 'asc' as const },
                    { key: 'popularity', label: 'Pop', dir: 'desc' as const },
                    { key: 'format', label: 'Format', dir: 'asc' as const },
                  ] as const).map(col => (
                    <th
                      key={col.label}
                      onClick={col.key ? () => toggleSort(col.key, col.dir) : undefined}
                      style={{ cursor: col.key ? 'pointer' : 'default', userSelect: 'none', whiteSpace: 'nowrap' }}
                    >
                      {col.label}
                      {col.key && <SortArrow column={col.key} activeCol={sortBy} activeDir={sortDir} />}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {data.data.map(r => (
                  <tr key={r.id} onClick={() => setSelected(r.id)}>
                    <td className="td-artist">{r.artists[0] ?? ''}</td>
                    <td className="td-title">
                      {r.title}
                      {r.in_watchlist && <span style={{ marginLeft: 6, fontSize: 9, color: 'var(--gold)', fontWeight: 600 }}>WL</span>}
                      {r.lowest_price_eur != null && <span style={{ marginLeft: 6 }}><PriceTag release={r} /></span>}
                    </td>
                    <td className="td-year">{r.year ?? ''}</td>
                    <td style={{ color: 'var(--text-muted)', fontSize: 12, maxWidth: 120, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {r.label ?? ''}
                    </td>
                    <td><CopyrightBadge status={r.copyright_status} /></td>
                    <td><PlatformBadges platforms={r.platforms} /></td>
                    <td><PopularityDot score={r.popularity_score} /></td>
                    <td style={{ fontSize: 11, color: 'var(--text-muted)' }}>{r.formats.slice(0,2).join(', ') || ''}</td>
                  </tr>
                ))}
                {data.data.length === 0 && (
                  <tr><td colSpan={8}>
                    <div className="empty-state">No releases found for these filters.</div>
                  </td></tr>
                )}
              </tbody>
            </table>
          </div>

          {data.total > LIMIT && (
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 14, justifyContent: 'flex-end' }}>
              <button className="toggle-btn" disabled={page === 0} onClick={() => setPage(p => p - 1)}>Prev</button>
              <span className="mono" style={{ fontSize: 12, color: 'var(--text-muted)' }}>
                {page * LIMIT + 1}–{Math.min((page + 1) * LIMIT, data.total)} / {data.total}
              </span>
              <button className="toggle-btn" disabled={(page + 1) * LIMIT >= data.total} onClick={() => setPage(p => p + 1)}>Next</button>
            </div>
          )}
        </>
      )}

      {selected && <DetailPanel releaseId={selected} onClose={() => setSelected(null)} />}
    </div>
  )
}
