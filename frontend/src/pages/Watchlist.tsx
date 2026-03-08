import { useRef, useEffect, useState } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { api, type WatchlistItem } from '../api'
import { ExternalLink } from 'lucide-react'
import {
  DndContext, useDroppable, useDraggable,
  PointerSensor, useSensor, useSensors,
  type DragEndEvent,
} from '@dnd-kit/core'
import { CSS } from '@dnd-kit/utilities'

const STATUSES = ['to_buy', 'ordered', 'purchased', 'ready_to_rip', 'ripping', 'done', 'skipped']
const STATUS_LABELS: Record<string, string> = {
  to_buy: 'To Buy',
  ordered: 'Ordered',
  purchased: 'Purchased',
  ready_to_rip: 'Ready to Rip',
  ripping: 'Ripping',
  done: 'Done',
  skipped: 'Skipped',
}
const STATUS_COLORS: Record<string, string> = {
  to_buy: '#8A8580',
  ordered: '#60a5fa',
  purchased: '#a78bfa',
  ready_to_rip: '#fbbf24',
  ripping: '#F59E0B',
  done: '#4ade80',
  skipped: '#64748b',
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
      marginTop: 5,
    }} />
  )
}

function PriceTag({ item }: { item: WatchlistItem }) {
  if (item.lowest_price_eur == null) return null
  const price = Number(item.lowest_price_eur)
  if (isNaN(price)) return null
  return (
    <span style={{
      fontSize: 10, fontFamily: 'var(--font-mono)',
      color: price <= 10 ? '#4ade80' : price <= 25 ? '#facc15' : '#f87171',
    }}>
      {'\u20AC'}{Math.round(price)}
      {item.num_for_sale != null && (
        <span style={{ color: 'var(--text-muted)', fontWeight: 400 }}> ({item.num_for_sale})</span>
      )}
    </span>
  )
}

function CardMenu({ item, onRemove }: { item: WatchlistItem; onRemove: (id: string) => void }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [open])

  return (
    <div ref={ref} style={{ position: 'relative', flexShrink: 0 }}>
      <button
        onClick={() => setOpen(o => !o)}
        style={{
          background: 'none', border: 'none', cursor: 'pointer',
          color: 'var(--text-muted)', padding: '2px 4px',
          fontSize: 14, lineHeight: 1, opacity: 0.5,
        }}
        title="More options"
      >...</button>
      {open && (
        <div style={{
          position: 'absolute', right: 0, top: '100%', zIndex: 10,
          background: 'var(--bg-raised)', border: '1px solid var(--border)',
          borderRadius: 6, padding: 4, minWidth: 130,
          boxShadow: '0 4px 12px rgba(0,0,0,0.3)',
        }}>
          <button
            onClick={() => {
              setOpen(false)
              if (window.confirm(`Remove "${item.title}" from watchlist?`)) onRemove(item.id)
            }}
            style={{
              display: 'flex', alignItems: 'center', gap: 6, width: '100%',
              padding: '6px 8px', borderRadius: 4, fontSize: 11,
              background: 'none', border: 'none', cursor: 'pointer',
              color: '#f87171', textAlign: 'left',
            }}
            onMouseEnter={e => (e.currentTarget.style.background = 'var(--bg-hover)')}
            onMouseLeave={e => (e.currentTarget.style.background = 'transparent')}
          >
            Remove from watchlist
          </button>
        </div>
      )}
    </div>
  )
}

function CardContent({ item, onRemove }: { item: WatchlistItem; onRemove?: (id: string) => void }) {
  return (
    <>
      <div style={{ display: 'flex', gap: 7, alignItems: 'flex-start' }}>
        <CopyrightDot status={item.copyright_status} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="kanban-artist">{item.artists[0] ?? 'Unknown'}</div>
          <div className="kanban-title">{item.title}</div>
          <div style={{ display: 'flex', gap: 6, alignItems: 'center', marginTop: 3 }}>
            <span className="kanban-year">{item.year ?? ''}</span>
            {item.label && (
              <span style={{ fontSize: 10, color: 'var(--text-muted)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 80 }}>
                {item.label}
              </span>
            )}
            <PriceTag item={item} />
          </div>
          {item.skip_reason && (
            <div style={{ fontSize: 9, color: '#64748b', marginTop: 3, fontStyle: 'italic' }}>
              {item.skip_reason.startsWith('found_on_streaming')
                ? 'Now on ' + item.skip_reason.replace('found_on_streaming: ', '')
                : item.skip_reason}
            </div>
          )}
        </div>
        <div style={{ display: 'flex', gap: 2, alignItems: 'center', flexShrink: 0 }}>
          {item.buy_url && (
            <a href={item.buy_url} target="_blank" rel="noreferrer"
              style={{ color: 'var(--text-muted)', padding: '2px' }}
              onClick={e => e.stopPropagation()}
              title="Buy on Discogs">
              <ExternalLink size={12} />
            </a>
          )}
          {onRemove && <CardMenu item={item} onRemove={onRemove} />}
        </div>
      </div>
    </>
  )
}

function DraggableCard({ item, onMove, onRemove }: { item: WatchlistItem; onMove: (id: string, status: string) => void; onRemove: (id: string) => void }) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({
    id: item.id,
    data: { item },
  })

  const idx = STATUSES.indexOf(item.status)
  const prevStatus = idx > 0 ? STATUSES[idx - 1] : undefined
  const nextStatus = idx < STATUSES.length - 1 ? STATUSES[idx + 1] : undefined
  const backStatus = item.status === 'skipped' ? 'to_buy' : prevStatus
  const backLabel = item.status === 'skipped' ? 'Restore' : backStatus ? STATUS_LABELS[backStatus] : undefined

  return (
    <div
      ref={setNodeRef}
      className="kanban-card"
      {...(isDragging ? { 'data-dragging': '' } : {})}
      style={{
        transform: transform ? CSS.Translate.toString(transform) : undefined,
        opacity: isDragging ? 0.85 : 1,
        cursor: isDragging ? 'grabbing' : 'grab',
        zIndex: isDragging ? 999 : undefined,
        position: isDragging ? 'relative' as const : undefined,
        boxShadow: isDragging ? '0 8px 24px rgba(0,0,0,0.4)' : undefined,
      }}
      {...listeners}
      {...attributes}
    >
      <CardContent item={item} onRemove={onRemove} />

      <div className="kanban-actions">
        {backStatus && backLabel && (
          <button
            onClick={() => onMove(item.id, backStatus)}
            className="kanban-move-btn kanban-move-btn--back"
            title={item.status === 'skipped' ? 'Restore to To Buy' : `Back to ${backLabel}`}
          >
            {backLabel}
          </button>
        )}
        <div style={{ flex: 1 }} />
        {nextStatus && item.status !== 'skipped' && (
          <button
            onClick={() => onMove(item.id, nextStatus)}
            className="kanban-move-btn kanban-move-btn--next"
            title={`Move to ${STATUS_LABELS[nextStatus]}`}
          >
            {STATUS_LABELS[nextStatus]}
          </button>
        )}
      </div>
    </div>
  )
}

function DroppableColumn({ status, items, onMove, onRemove, isOver }: {
  status: string; items: WatchlistItem[]
  onMove: (id: string, status: string) => void
  onRemove: (id: string) => void
  isOver: boolean
}) {
  const { setNodeRef } = useDroppable({ id: status })
  const color = STATUS_COLORS[status]

  return (
    <div
      ref={setNodeRef}
      className="kanban-col"
      style={{
        outline: isOver ? `2px solid ${color}` : undefined,
        outlineOffset: -2,
        borderRadius: 8,
        transition: 'outline 0.15s',
      }}
    >
      <div className="kanban-col-header">
        <span style={{ color }}>{STATUS_LABELS[status]}</span>
        <span className="kanban-count">{items.length}</span>
      </div>
      <div style={{ width: '100%', height: 2, background: color, borderRadius: 1, opacity: 0.5, marginBottom: 2 }} />
      {items.length === 0 && (
        <div style={{ color: 'var(--text-muted)', fontSize: 10, textAlign: 'center', padding: '16px 0', opacity: 0.3 }}>
          empty
        </div>
      )}
      {items.map(item => (
        <DraggableCard
          key={item.id}
          item={item}
          onMove={onMove}
          onRemove={onRemove}
        />
      ))}
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
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['watchlist'] })
      qc.invalidateQueries({ queryKey: ['releases'] })
    },
  })

  const removeMut = useMutation({
    mutationFn: (id: string) => api.deleteWatchlistItem(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['watchlist'] })
      qc.invalidateQueries({ queryKey: ['releases'] })
    },
  })

  const [overColumn, setOverColumn] = useState<string | null>(null)

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } })
  )

  const handleDragOver = (event: { over: { id: string | number } | null }) => {
    setOverColumn(event.over ? String(event.over.id) : null)
  }

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event
    setOverColumn(null)
    if (!over) return
    const itemId = String(active.id)
    const newStatus = String(over.id)
    const item = data?.find(w => w.id === itemId)
    if (item && item.status !== newStatus && STATUSES.includes(newStatus)) {
      moveMut.mutate({ id: itemId, status: newStatus })
    }
  }

  const byStatus = (status: string) => data?.filter(w => w.status === status) ?? []
  const totalItems = data?.length ?? 0
  const activeStatuses = STATUSES.filter(s => s !== 'skipped')
  const skippedItems = byStatus('skipped')

  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">WATCHLIST</h1>
        <p className="page-subtitle">
          {totalItems > 0
            ? `${totalItems} item${totalItems !== 1 ? 's' : ''}, prices auto-checked via Discogs`
            : 'Track purchases and ripping progress'
          }
        </p>
      </div>

      {isLoading && <div className="loading-ring" />}
      {error && <div className="error-msg">Cannot reach API - is mm-api running?</div>}

      {data && totalItems > 0 && (
        <>
          <DndContext
            sensors={sensors}
            onDragOver={handleDragOver}
            onDragEnd={handleDragEnd}
          >
            <div className="kanban-board">
              {activeStatuses.map(status => (
                <DroppableColumn
                  key={status}
                  status={status}
                  items={byStatus(status)}
                  onMove={(id, s) => moveMut.mutate({ id, status: s })}
                  onRemove={id => removeMut.mutate(id)}
                  isOver={overColumn === status}
                />
              ))}
            </div>


          {skippedItems.length > 0 && (
            <details style={{ marginTop: 16 }}>
              <summary style={{
                cursor: 'pointer', fontSize: 12, color: '#64748b',
                letterSpacing: '0.06em', padding: '8px 0',
              }}>
                SKIPPED ({skippedItems.length}), items found on streaming or manually skipped
              </summary>
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))', gap: 8, marginTop: 8 }}>
                {skippedItems.map(item => (
                  <DraggableCard
                    key={item.id}
                    item={item}
                    onMove={(id, s) => moveMut.mutate({ id, status: s })}
                    onRemove={id => removeMut.mutate(id)}
                  />
                ))}
              </div>
            </details>
          )}
          </DndContext>
        </>
      )}

      {data && totalItems === 0 && (
        <div className="empty-state">
          <Disc3Icon />
          <span>No items on watchlist yet.</span>
          <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
            Add releases from the <a href="/releases" style={{ color: 'var(--gold)' }}>Releases</a> page, or run:
          </span>
          <code className="mono" style={{ background: 'var(--bg-hover)', padding: '4px 10px', borderRadius: 4, fontSize: 12 }}>
            mm search --auto-watchlist
          </code>
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
