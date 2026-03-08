const BASE = import.meta.env.VITE_API_URL ?? ''

async function get<T>(path: string, params?: Record<string, string | number | boolean | undefined>): Promise<T> {
  let url = BASE + path
  if (params) {
    const qs = new URLSearchParams()
    Object.entries(params).forEach(([k, v]) => {
      if (v !== undefined) qs.set(k, String(v))
    })
    const str = qs.toString()
    if (str) url += '?' + str
  }
  const res = await fetch(url)
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`)
  return res.json()
}

async function patch<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`)
  return res.json()
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`)
  return res.json()
}

// ─── Types ────────────────────────────────────────────────────────────────────

export interface Stats {
  total_releases: number
  missing_from_streaming: number
  public_domain: number
  watchlist_total: number
  watchlist_to_buy: number
  watchlist_done: number
  tracks_digitized: number
  rip_jobs_done: number
}

export interface PlatformCheck {
  platform: string
  found: boolean
  error: boolean
  match_score: number | null
  platform_url: string | null
}

export interface Release {
  id: string
  discogs_id: number
  title: string
  artists: string[]
  label: string | null
  country_code: string
  year: number | null
  genres: string[]
  formats: string[]
  discogs_url: string
  buy_url: string
  copyright_status: string
  platforms: PlatformCheck[]
  in_watchlist?: boolean
  watchlist_id?: string
  watchlist_status?: string
  lowest_price_eur: number | null
  num_for_sale: number | null
  popularity_score: number | null
  discogs_want: number | null
  discogs_have: number | null
  discogs_rating: number | null
  discogs_rating_count: number | null
  lastfm_listeners: number | null
  lastfm_playcount: number | null
  has_wikipedia: boolean | null
}

export interface ReleasesResponse {
  data: Release[]
  total: number
  limit: number
  offset: number
}

export interface WatchlistItem {
  id: string
  release_id: string
  status: string
  buy_url: string | null
  notes: string | null
  added_at: string
  title: string
  artists: string[]
  year: number | null
  label: string | null
  copyright_status: string
  discogs_url: string
  lowest_price_eur: number | null
  num_for_sale: number | null
  skip_reason: string | null
}

export interface TrackCheck {
  track_title: string
  track_number: number | null
  platform: string
  found: boolean
  match_score: number | null
  platform_url: string | null
}

export interface RipJob {
  id: string
  status: string
  drive_path: string
  backend: string
  track_count: number | null
  output_dir: string | null
  error_msg: string | null
  accuraterip_status: string | null
  started_at: string
  finished_at: string | null
  release_title: string | null
}

// ─── Discovery types ──────────────────────────────────────────────────────────

export interface DiscoveryJob {
  id: string
  status: 'running' | 'paused' | 'completed' | 'failed' | 'cancelled'
  country: string
  country_code: string
  genres: string[]
  year_from: number
  year_to: number
  format_filter: string
  current_page: number
  total_pages: number | null
  releases_saved: number
  missing_count: number
  started_at: string
  finished_at: string | null
  error_msg: string | null
  max_pages: number | null
}

export interface StartJobRequest {
  country?: string
  country_code?: string
  genres?: string[]
  year_from?: number
  year_to?: number
  format_filter?: string
  max_pages?: number
}

// ─── API calls ────────────────────────────────────────────────────────────────

export interface PlatformCheckerStatus {
  active: boolean
  unchecked_count: number
  total_checked: number
  active_platforms: string[]
  skipped_platforms: string[]
}

export interface ReleasesFilter {
  country?: string
  missing_only?: boolean
  year_from?: number
  year_to?: number
  copyright_status?: string
  format_type?: string  // "Album" | "Single" | "EP" | "Compilation" | ""
  media?: string        // "Vinyl" | "CD" | "Cassette" | ""
  limit?: number
  offset?: number
  platform_status?: string   // "unchecked" | "missing" | "found" | ""
  platforms?: string         // comma-separated: "spotify,deezer"
  sort_by?: string           // "artist" | "title" | "year" | "label" | "copyright" | "popularity" | "format" | "price" + optional "_asc"/"_desc"
}

export const api = {
  stats: () => get<Stats>('/api/stats'),
  releases: (f?: ReleasesFilter) => get<ReleasesResponse>('/api/releases', f as Record<string, string | number | boolean | undefined>),
  release: (id: string) => get<Release>(`/api/releases/${id}`),
  watchlist: () => get<WatchlistItem[]>('/api/watchlist'),
  updateWatchlistStatus: (id: string, status: string) =>
    patch(`/api/watchlist/${id}/status`, { status }),
  deleteWatchlistItem: (id: string) =>
    fetch(BASE + `/api/watchlist/${id}`, { method: 'DELETE' }).then(r => r.json()),
  addToWatchlist: (discogs_id: number, notes?: string) =>
    post('/api/watchlist', { discogs_id, notes }),
  ripJobs: () => get<RipJob[]>('/api/rip-jobs'),
  releaseTracks: (id: string) => get<TrackCheck[]>(`/api/releases/${id}/tracks`),
  discoveryJobs: () => get<DiscoveryJob[]>('/api/discovery/jobs'),
  discoveryCreate: (req: StartJobRequest) => post<{ id: string }>('/api/discovery/jobs', req),
  discoveryResume: (id: string) => post<{ id: string }>(`/api/discovery/jobs/${id}/resume`, {}),
  discoveryPause: (id: string) => post<{ ok: boolean }>(`/api/discovery/jobs/${id}/pause`, {}),
  discoveryDeleteJob: (id: string) => fetch(BASE + `/api/discovery/jobs/${id}`, { method: 'DELETE' }).then(r => r.json()),
  exportReleases: async () => {
    const res = await fetch(BASE + '/api/releases/export')
    if (!res.ok) throw new Error('Export failed')
    const blob = await res.blob()
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'releases.json'
    a.click()
    URL.revokeObjectURL(url)
  },
  importReleases: async (file: File) => {
    const text = await file.text()
    const res = await fetch(BASE + '/api/releases/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: text,
    })
    if (!res.ok) throw new Error('Import failed')
    return res.json() as Promise<{ imported: number }>
  },
  clearReleases: async () => {
    const res = await fetch(BASE + '/api/releases/clear', { method: 'DELETE' })
    if (!res.ok) {
      const msg = await res.text().catch(() => res.statusText)
      throw new Error(msg)
    }
    return res.json()
  },
  platformCheckerStatus: () => get<PlatformCheckerStatus>('/api/platform-checker/status'),
  platformCheckerStart: () => post<{ ok: boolean }>('/api/platform-checker/start', {}),
  platformCheckerStop: () => post<{ ok: boolean }>('/api/platform-checker/stop', {}),
  enrichReleases: () => post<{ started: boolean; releases_to_enrich: number; message: string }>('/api/releases/enrich', {}),
}
