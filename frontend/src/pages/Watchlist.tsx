import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api, type WatchlistItem } from '../api'
import { ExternalLink } from 'lucide-react'

const STATUSES = ['to_buy', 'ordered', 'purchased', 'ready_to_rip', 'ripping', 'done']
const STATUS_LABELS: Record<string, string> = {
  to_buy: 'To Buy',
  ordered: 'Ordered',
  purchased: 'Purchased',
  ready_to_rip: 'Ready to Rip',
  ripping: 'Ripping',
  done: 'Done',
}
const STATUS_COLORS: Record<string, string> = {
  to_buy: '#8A8580',
  ordered: '#60a5fa',
  purchased: '#a78bfa',
  ready_to_rip: '#fbbf24',
  ripping: '#F59E0B',
  done: '#4ade80',
}

function CopyrightDot({ status }: { status: string }) {
  const colors: Record<string, string> = {
    PUBLIC_DOMAIN: '#4ade80',
    LIKELY_PUBLIC_DOMAIN: '#2dd4bf',
    CHECK_REQUIRED: '#fbbf24',
    UNDER_COPYRIGHT: '#f87171',
    UNKNOWN: '#555',
  }
  return (
    <span title={status} style={{
      width: 6, height: 6, borderRadius: '50%',
      background: colors[status] ?? '#555',
      display: 'inline-block', flexShrink: 0,
    }} />
  )
}

function KanbanCard({ item, onMove }: { item: WatchlistItem; onMove: (id: string, status: string) => void }) {
  const nextStatus = STATUSES[STATUSES.indexOf(item.status) + 1]

  return (
    <div className="kanban-card">
      <div style={{ display: 'flex', gap: 6, alignItems: 'flex-start' }}>
        <CopyrightDot status={item.copyright_status} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="kanban-artist">{item.artists[0] ?? '—'}</div>
          <div className="kanban-title">{item.title}</div>
          <div className="kanban-year">{item.year ?? '—'}</div>
        </div>
      </div>
      <div style={{ display: 'flex', gap: 4, marginTop: 4 }}>
        {item.buy_url && (
          <a href={item.buy_url} target="_blank" rel="noreferrer"
            className="badge badge--unchecked"
            style={{ gap: 4, fontSize: 10 }}
            onClick={e => e.stopPropagation()}>
            <ExternalLink size={9} /> Buy
          </a>
        )}
        {nextStatus && (
          <button
            onClick={() => onMove(item.id, nextStatus)}
            className="badge badge--CHECK_REQUIRED"
            style={{ border: 'none', fontSize: 10, cursor: 'pointer', gap: 4 }}>
            → {STATUS_LABELS[nextStatus]}
          </button>
        )}
      </div>
    </div>
  )
}

export default function Watchlist() {
  const qc = useQueryClient()
  const { data, isLoading, error } = useQuery<WatchlistItem[]>({
    queryKey: ['watchlist'],
    queryFn: api.watchlist,
    refetchInterval: 15_000,
  })

  const moveMut = useMutation({
    mutationFn: ({ id, status }: { id: string; status: string }) =>
      api.updateWatchlistStatus(id, status),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['watchlist'] }),
  })

  const byStatus = (status: string) => data?.filter(w => w.status === status) ?? []

  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">WATCHLIST</h1>
        <p className="page-subtitle">
          {data ? `${data.length} items - click "→ Status" to advance through pipeline` : 'Track purchases and ripping progress'}
        </p>
      </div>

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">⚠ API error - is mm-api running?</div>}

      {data && (
        <div className="kanban-board">
          {STATUSES.map(status => {
            const items = byStatus(status)
            const color = STATUS_COLORS[status]
            return (
              <div key={status} className="kanban-col">
                <div className="kanban-col-header">
                  <span style={{ color }}>{STATUS_LABELS[status]}</span>
                  <span className="kanban-count">{items.length}</span>
                </div>
                <div style={{ width: '100%', height: 2, background: color, borderRadius: 1, opacity: 0.5, marginBottom: 4 }} />
                {items.length === 0 && (
                  <div style={{ color: 'var(--text-muted)', fontSize: 11, textAlign: 'center', padding: '12px 0' }}>
                    empty
                  </div>
                )}
                {items.map(item => (
                  <KanbanCard
                    key={item.id}
                    item={item}
                    onMove={(id, s) => moveMut.mutate({ id, status: s })}
                  />
                ))}
              </div>
            )
          })}
        </div>
      )}

      {data && data.length === 0 && (
        <div className="empty-state">
          <Disc3Icon />
          <span>No items on watchlist yet.</span>
          <span style={{ fontSize: 12 }}>Run <code className="mono" style={{ background: 'var(--bg-hover)', padding: '2px 6px', borderRadius: 4 }}>mm search --auto-watchlist</code> to populate</span>
        </div>
      )}
    </div>
  )
}

function Disc3Icon() {
  return (
    <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>
      <path d="M12 2a10 10 0 0 1 7.38 16.78"/><path d="m16.67 19.52-.93-2.79"/>
    </svg>
  )
}
