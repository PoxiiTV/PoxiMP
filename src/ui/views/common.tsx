// Pieces shared by the phase-4 views: the skin's quality badge (context), a virtual track list with
// pointer-driven reordering, small form controls and the file dialogs.
import { createContext, memo, useContext, useLayoutEffect, useRef, useState, type ComponentType, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { native } from '../../core/api'
import { formatLabel, time } from '../../core/format'
import { useT } from '../../core/i18n'
import { useStore } from '../../core/store'
import type { Track } from '../../core/types'
import { useBadge } from '../quality'
import * as Ico from '../shell/icons'
import { isMenuKey, openTrackMenu, type MenuItem } from '../TrackMenu'
import { VirtualList } from '../VirtualList'

/** Each skin passes its own badge so the shared lists show the signature element in its language. */
export const BadgeSlot = createContext<ComponentType<{ track: Track }> | null>(null)

function PlainBadge({ track }: { track: Track }) {
  const b = useBadge(track)
  if (!b) return null
  return (
    <span className={`vw-qb q-${b.kind}`}>
      {b.label}
      {b.extra && <small> {b.extra}</small>}
    </span>
  )
}

export function QualityBadge({ track }: { track: Track }) {
  const B = useContext(BadgeSlot) ?? PlainBadge
  return <B track={track} />
}

/** Reads a px length the skin sets in CSS (e.g. --vw-row-h) so JS row maths follows the skin. */
export function useCssPx(name: string, fallback: number) {
  const ref = useRef<HTMLDivElement>(null)
  const [v, setV] = useState(fallback)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const n = parseFloat(getComputedStyle(el).getPropertyValue(name))
    if (n > 0 && n !== v) setV(n)
  }, [name, v])
  return [ref, v] as const
}

// ---------- track list ----------

export interface TrackListProps {
  total: number
  rows: (Track | undefined)[]
  ensureRange: (start: number, end: number) => void
  label: string
  /** Row index that is playing (queue) — otherwise rows matching the current track are marked */
  playingIndex?: number
  onPlay: (i: number, t: Track) => void
  onRemove?: (i: number) => void
  onMove?: (from: number, to: number) => void
  menuExtra?: (i: number, t: Track) => MenuItem[]
  /** Dim rows before this one (queue: already played) */
  playedBefore?: number
  album?: boolean
  /** Row to scroll into view once (e.g. the current track in the queue) */
  reveal?: number
  className?: string
}

/** Drag state: `slot` = gap the row would land in (0..total), y = pointer within the list. */
interface Drag {
  from: number
  slot: number
  y: number
  title: string
}

export function TrackList({ total, rows, ensureRange, label, playingIndex, onPlay, onRemove, onMove, menuExtra, playedBefore = -1, album = true, reveal, className = '' }: TrackListProps) {
  const t = useT()
  const [box, rowH] = useCssPx('--vw-row-h', 44)
  const current = useStore((s) => s.track)
  const [sel, setSel] = useState(-1)
  const [drag, setDrag] = useState<Drag | null>(null)
  const dragRef = useRef<Drag | null>(null)
  const scroller = useRef<HTMLElement | null>(null)
  const auto = useRef(0)
  const idBase = useRef(`vw${Math.random().toString(36).slice(2, 7)}`).current
  const active = sel >= 0 ? sel : (reveal ?? -1)

  const slotAt = (clientY: number): number => {
    const el = scroller.current!
    const r = el.getBoundingClientRect()
    return Math.max(0, Math.min(total, Math.round((clientY - r.top + el.scrollTop) / rowH)))
  }
  const setD = (d: Drag | null): void => {
    dragRef.current = d
    setDrag(d)
  }

  const startDrag = (e: PointerEvent<HTMLElement>, i: number, tr: Track): void => {
    if (e.button !== 0 || !onMove) return
    e.preventDefault()
    const el = e.currentTarget.closest<HTMLElement>('.vw-scroll')!
    scroller.current = el
    el.setPointerCapture(e.pointerId)
    const r = el.getBoundingClientRect()
    setSel(i)
    setD({ from: i, slot: i, y: e.clientY - r.top, title: tr.title })
    // Auto-scroll near the edges while held still
    let lastY = e.clientY
    el.onpointermove = (ev) => {
      lastY = ev.clientY
      const d = dragRef.current
      if (d) setD({ ...d, slot: slotAt(ev.clientY), y: ev.clientY - el.getBoundingClientRect().top })
    }
    const tick = (): void => {
      const rr = el.getBoundingClientRect()
      const edge = Math.min(48, rr.height / 4)
      const dy = lastY < rr.top + edge ? -(rr.top + edge - lastY) / 4 : lastY > rr.bottom - edge ? (lastY - rr.bottom + edge) / 4 : 0
      if (dy) {
        el.scrollTop += dy
        const d = dragRef.current
        if (d) setD({ ...d, slot: slotAt(lastY) })
      }
      auto.current = requestAnimationFrame(tick)
    }
    auto.current = requestAnimationFrame(tick)
    const end = (ev: globalThis.PointerEvent): void => {
      cancelAnimationFrame(auto.current)
      el.onpointermove = el.onpointerup = el.onpointercancel = null
      const d = dragRef.current
      setD(null)
      if (!d || ev.type === 'pointercancel') return
      const to = d.slot > d.from ? d.slot - 1 : d.slot
      if (to !== d.from) {
        onMove(d.from, to)
        setSel(to)
      }
    }
    el.onpointerup = el.onpointercancel = end
  }

  const onKey = (e: KeyboardEvent<HTMLDivElement>): void => {
    const cur = active
    const tr = rows[cur]
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const dir = e.key === 'ArrowDown' ? 1 : -1
      const n = Math.max(0, Math.min(total - 1, cur + dir))
      if (e.altKey && onMove && cur >= 0 && n !== cur) onMove(cur, n)
      setSel(n)
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault()
      setSel(e.key === 'Home' ? 0 : total - 1)
    } else if (e.key === 'Enter' && tr) onPlay(cur, tr)
    else if ((e.key === 'Delete' || e.key === 'Backspace') && onRemove && tr) {
      onRemove(cur)
      setSel(Math.min(cur, total - 2))
    } else if (isMenuKey(e) && tr) {
      e.preventDefault()
      const row = document.getElementById(`${idBase}-${cur}`)
      if (row) openTrackMenu(row, tr, { onPlay: () => onPlay(cur, tr), extra: menuExtra?.(cur, tr) })
    }
  }

  return (
    <div ref={box} className={`vw-tracks ${onMove ? 'can-move' : ''} ${onRemove ? 'can-x' : ''} ${album ? '' : 'no-al'} ${drag ? 'dragging' : ''} ${className}`}>
      <div className="vw-tr vw-th" aria-hidden="true">
        {onMove && <span className="c-h" />}
        <span className="c-n">{t('library.cols.n')}</span>
        <span className="c-ti">{t('library.cols.title')}</span>
        {album && <span className="c-al">{t('views.cols.album')}</span>}
        <span className="c-du">{t('library.cols.duration')}</span>
        <span className="c-fm">{t('library.cols.format')}</span>
        <span className="c-q">{t('library.cols.quality')}</span>
        {onRemove && <span className="c-x" />}
      </div>
      <VirtualList
        className="vw-scroll"
        count={total}
        rowHeight={rowH}
        onRange={ensureRange}
        active={active}
        role="listbox"
        tabIndex={0}
        aria-label={label}
        aria-activedescendant={active >= 0 && rows[active] ? `${idBase}-${active}` : undefined}
        onKeyDown={onKey}
      >
        {(i) => (
          <Row
            i={i}
            id={`${idBase}-${i}`}
            track={rows[i]}
            sel={i === active}
            playing={playingIndex !== undefined ? i === playingIndex : !!current && rows[i]?.id === current.id}
            played={i < playedBefore}
            lifted={drag?.from === i}
            album={album}
            onSelect={setSel}
            onPlay={onPlay}
            onRemove={onRemove}
            onGrab={onMove ? startDrag : undefined}
            menuExtra={menuExtra}
          />
        )}
      </VirtualList>
      {drag && (
        <>
          <div className="vw-drop" style={{ top: (scroller.current?.offsetTop ?? 0) + drag.slot * rowH - (scroller.current?.scrollTop ?? 0) }} aria-hidden="true" />
          <div className="vw-ghost" style={{ top: (scroller.current?.offsetTop ?? 0) + drag.y }} aria-hidden="true">
            {drag.title}
          </div>
        </>
      )}
    </div>
  )
}

interface RowProps {
  i: number
  id: string
  track?: Track
  sel: boolean
  playing: boolean
  played: boolean
  lifted: boolean
  album: boolean
  onSelect: (i: number) => void
  onPlay: (i: number, t: Track) => void
  onRemove?: (i: number) => void
  onGrab?: (e: PointerEvent<HTMLElement>, i: number, t: Track) => void
  menuExtra?: (i: number, t: Track) => MenuItem[]
}

const Row = memo(function Row({ i, id, track, sel, playing, played, lifted, album, onSelect, onPlay, onRemove, onGrab, menuExtra }: RowProps) {
  const t = useT()
  if (!track)
    return (
      <div className="vw-tr vw-row vw-skel" aria-hidden="true">
        <span className="c-ti">
          <i />
        </span>
      </div>
    )
  return (
    <div
      id={id}
      role="option"
      aria-selected={sel}
      aria-current={playing || undefined}
      className={`vw-tr vw-row ${sel ? 'sel' : ''} ${playing ? 'is-cur' : ''} ${played ? 'played' : ''} ${lifted ? 'lifted' : ''}`}
      onClick={() => onSelect(i)}
      onDoubleClick={() => onPlay(i, track)}
      onContextMenu={(e) => {
        onSelect(i)
        openTrackMenu(e, track, { onPlay: () => onPlay(i, track), extra: menuExtra?.(i, track) })
      }}
    >
      {onGrab && (
        <span className="c-h" title={t('views.dragHint')} onPointerDown={(e) => onGrab(e, i, track)}>
          <Ico.Drag />
        </span>
      )}
      <span className="c-n">
        {playing ? (
          <span className="vw-eq" aria-label={t('player.nowPlaying')}>
            <i />
            <i />
            <i />
          </span>
        ) : (
          i + 1
        )}
      </span>
      <span className="c-ti">
        <b>{track.title}</b>
        <span>{track.artist}</span>
      </span>
      {album && <span className="c-al">{track.album}</span>}
      <span className="c-du">{time(track.duration)}</span>
      <span className="c-fm">{formatLabel(track)}</span>
      <span className="c-q">
        <QualityBadge track={track} />
      </span>
      {onRemove && (
        <span className="c-x">
          <button
            className="vw-x"
            tabIndex={-1}
            aria-label={t('views.remove', { title: track.title })}
            onClick={(e) => {
              e.stopPropagation()
              onRemove(i)
            }}
          >
            <Ico.Close />
          </button>
        </span>
      )}
    </div>
  )
})

// ---------- bits ----------

export function ViewHead({ kicker, title, meta, children }: { kicker?: ReactNode; title: ReactNode; meta?: ReactNode; children?: ReactNode }) {
  return (
    <header className="vw-head">
      <div className="vw-head-t">
        {kicker && <span className="vw-kicker">{kicker}</span>}
        <h2 className="vw-title">{title}</h2>
        {meta && <div className="vw-meta">{meta}</div>}
      </div>
      {children && <div className="vw-actions">{children}</div>}
    </header>
  )
}

export function Switch({ checked, onChange, children }: { checked: boolean; onChange: (b: boolean) => void; children: ReactNode }) {
  return (
    <label className="vw-switch">
      <input type="checkbox" role="switch" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <i aria-hidden="true" />
      <span>{children}</span>
    </label>
  )
}

export function Empty({ title, hint, children }: { title: string; hint?: string; children?: ReactNode }) {
  return (
    <div className="vw-empty">
      <h3>{title}</h3>
      {hint && <p>{hint}</p>}
      {children}
    </div>
  )
}

// ---------- file dialogs (outside Tauri: a demo path so the preview still runs) ----------

export async function pickFile(name: string, extensions: string[]): Promise<string | null> {
  if (!native) return `C:/Users/Demo/Music/demo.${extensions[0]}`
  const p = await open({ multiple: false, directory: false, filters: [{ name, extensions }] })
  return typeof p === 'string' ? p : null
}

export async function saveFile(name: string, extensions: string[], defaultPath: string): Promise<string | null> {
  if (!native) return `C:/Users/Demo/Music/${defaultPath}`
  return save({ defaultPath, filters: [{ name, extensions }] })
}

/** "Para el coche" → "Para el coche" safe as a file name */
export const fileName = (s: string): string => s.replace(/[\\/:*?"<>|]+/g, ' ').trim() || 'PoxiMP'
