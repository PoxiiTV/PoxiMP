// Global search bar, shared by every skin. Each Shell keeps its own input markup (icon, clear
// button, kbd hint) and spreads `input` onto it; `popup` is the results dropdown (fixed, under the
// input, styled like the track menu so it inherits the skin's tokens). The text is still the
// library filter (store.search): Enter on nothing picked opens the filtered library.
// In mini (docs/bocetos/11-buscador-mini.html) the field sits in the player and the results are a
// panel under it at full width: the window grows to show them (core/window.ts growMini).
import { useEffect, useId, useLayoutEffect, useRef, useState, type ChangeEvent, type KeyboardEvent, type MouseEvent, type ReactNode, type RefObject } from 'react'
import { api, coverUrl } from '../core/api'
import { useT } from '../core/i18n'
import { setState, useStore } from '../core/store'
import type { SearchGroup, SearchHit } from '../core/types'
import { growMini, leaveMini } from '../core/window'
import * as Ico from './shell/icons'
import { openTrackMenu } from './TrackMenu'
import './views/views-base.css'

const DEBOUNCE_MS = 140
/** Same cap as the backend uses */
const MAX_QUERY = 200
/** Tallest the mini's results panel gets (it scrolls beyond) */
const MINI_RESULTS_MAX = 360

let current: HTMLInputElement | null = null

/** Focuses the search bar of the active skin (Ctrl/⌘ K and "/" do it already). */
export function focusSearch(): void {
  current?.focus()
  current?.select()
}

interface Item {
  group: SearchGroup
  hit: SearchHit
}

function activate(item: Item | null): void {
  if (!item) return setState({ view: 'library' })
  const { group, hit } = item
  if (group.kind === 'tracks') void api.playTrack(Number(hit.id)).catch(() => {})
  else if (group.kind === 'playlists') setState({ view: 'playlists', playlist: Number(hit.id) })
  else setState({ view: 'browse', browse: { kind: group.kind === 'albums' ? 'album' : 'artist', id: hit.id, title: hit.title } })
}

/** bar = the title bar's (full window); mini = the player's in mini mode; off = mounted but idle
 *  (the player keeps its hook in the full window, where the title bar owns the search). */
export type SearchPlace = 'bar' | 'mini' | 'off'

export function useGlobalSearch(place: SearchPlace = 'bar') {
  const q = useStore((s) => s.search)
  const ref = useRef<HTMLInputElement>(null)
  const listId = useId()
  const [open, setOpen] = useState(false)
  const [groups, setGroups] = useState<SearchGroup[]>([])
  const [loading, setLoading] = useState(false)
  const [active, setActive] = useState(-1)

  const mini = place === 'mini'
  // One search bar per window: register it and own the shortcuts
  useEffect(() => {
    if (place === 'off') return
    const el = ref.current
    current = el
    const onKey = (e: globalThis.KeyboardEvent): void => {
      const target = e.target as HTMLElement
      const k = (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k'
      const slash = e.key === '/' && !e.ctrlKey && !e.metaKey && !e.altKey && !target.closest('input, textarea, select, [contenteditable=true]')
      if (!k && !slash) return
      e.preventDefault()
      focusSearch()
    }
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('keydown', onKey)
      if (current === el) current = null
      // Its field goes away with the mode (no blur event then): do not reopen on the way back
      setOpen(false)
    }
  }, [place])

  const query = q.trim().slice(0, MAX_QUERY)
  useEffect(() => {
    if (!open || !query) {
      setGroups([])
      setLoading(false)
      return
    }
    let alive = true
    setLoading(true)
    const id = setTimeout(() => {
      api.searchAll(query).then(
        (g) => {
          if (!alive) return
          setGroups(g)
          // Mini has no "see all": the first result is the one Enter plays
          setActive(mini && g.some((x) => x.hits.length) ? 0 : -1)
          setLoading(false)
        },
        () => alive && (setGroups([]), setLoading(false))
      )
    }, DEBOUNCE_MS)
    return () => {
      alive = false
      clearTimeout(id)
    }
  }, [query, open, mini])

  const items: Item[] = groups.flatMap((group) => group.hits.map((hit) => ({ group, hit })))
  const shown = open && !!query && place !== 'off'
  // Last option = "see all in the library" (not in mini: the library is not on screen)
  const count = Math.max(1, items.length + (mini ? 0 : 1))

  const close = (): void => {
    setOpen(false)
    setActive(-1)
    ref.current?.blur()
  }
  const pick = (i: number): void => {
    const item = items[i] ?? null
    // Mini: a song plays right there; an album, artist or list opens in the full window
    if (!mini) activate(item)
    else if (item?.group.kind === 'tracks') activate(item)
    else if (item) void leaveMini().then(() => activate(item)).catch(() => {})
    close()
  }

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      setOpen(true)
      setActive((a) => (e.key === 'ArrowDown' ? (a + 1) % count : a <= 0 ? count - 1 : a - 1))
    } else if (e.key === 'Enter') pick(active >= 0 ? active : mini ? 0 : items.length)
    else if (e.key === 'Escape') {
      setState({ search: '' })
      close()
    } else return
    e.preventDefault()
    e.stopPropagation()
  }

  const input = {
    ref,
    value: q,
    maxLength: MAX_QUERY,
    autoComplete: 'off',
    role: 'combobox',
    'aria-expanded': shown,
    'aria-controls': listId,
    'aria-autocomplete': 'list' as const,
    'aria-activedescendant': shown && active >= 0 ? `${listId}-${active}` : undefined,
    onChange: (e: ChangeEvent<HTMLInputElement>) => {
      setState({ search: e.target.value })
      setOpen(true)
      setActive(-1)
    },
    onKeyDown,
    onFocus: () => setOpen(true),
    onBlur: () => setOpen(false)
  }
  const popup = shown ? <Popup anchor={ref} id={listId} inline={mini} query={query} groups={groups} items={items} loading={loading} active={active} onHover={setActive} onPick={pick} /> : null
  return { input, popup }
}

interface PopupProps {
  anchor: RefObject<HTMLInputElement | null>
  id: string
  /** Mini: a panel in the page flow under the player (no "see all"), not a dropdown */
  inline: boolean
  query: string
  groups: SearchGroup[]
  items: Item[]
  loading: boolean
  active: number
  onHover: (i: number) => void
  onPick: (i: number) => void
}

function Popup({ anchor, id, inline, query, groups, items, loading, active, onHover, onPick }: PopupProps) {
  const t = useT()
  const ref = useRef<HTMLDivElement>(null)
  const inner = useRef<HTMLDivElement>(null)
  const [box, setBox] = useState<{ left: number; top: number; width: number } | null>(null)

  // Under the search field (its label, if the skin wraps it), kept inside the window
  useLayoutEffect(() => {
    if (inline) return
    const place = (): void => {
      const el = anchor.current
      if (!el) return
      const r = (el.closest('label') ?? el).getBoundingClientRect()
      const width = Math.min(Math.max(r.width, 380), innerWidth - 16)
      setBox({ left: Math.max(8, Math.min(r.left, innerWidth - width - 8)), top: r.bottom + 6, width })
    }
    place()
    window.addEventListener('resize', place)
    return () => window.removeEventListener('resize', place)
  }, [anchor, inline])

  // Mini: the window grows by the panel's height while it shows, and shrinks back when it goes
  useLayoutEffect(() => {
    const el = inner.current
    if (!inline || !el) return
    let last = -1
    const ro = new ResizeObserver(() => {
      const h = Math.min(MINI_RESULTS_MAX, Math.ceil(el.getBoundingClientRect().height))
      if (h !== last) void growMini((last = h))
    })
    ro.observe(el)
    return () => {
      ro.disconnect()
      void growMini(0)
    }
  }, [inline])

  // Keep the active row in view, scrolling only the list (scrollIntoView would also scroll the
  // page around the mini panel while the window has not grown yet)
  useEffect(() => {
    const list = ref.current
    const row = list?.querySelector(`[id="${id}-${active}"]`)
    if (!list || !row) return
    const a = list.getBoundingClientRect()
    const b = row.getBoundingClientRect()
    if (b.top < a.top) list.scrollTop -= a.top - b.top
    else if (b.bottom > a.bottom) list.scrollTop += b.bottom - a.bottom
  }, [active, id])

  const sub = (g: SearchGroup, h: SearchHit): string => {
    const songs = h.count === 1 ? t('library.song') : t('library.songs', { n: h.count ?? 0 })
    if (g.kind === 'tracks') return h.subtitle ?? ''
    if (g.kind === 'albums') return `${h.subtitle ?? t('search.various')} · ${songs}`
    return songs
  }

  let i = -1
  const option = (n: number, key: string, cls: string, body: ReactNode, extra: Record<string, unknown> = {}) => (
    <div key={key} id={`${id}-${n}`} role="option" aria-selected={n === active} className={`tmenu-i gs-i ${cls} ${n === active ? 'on' : ''}`} onPointerEnter={() => onHover(n)} onClick={() => onPick(n)} {...extra}>
      {body}
    </div>
  )

  const body = (
    <>
      {groups.map((g) => (
        <div key={`${g.source}:${g.kind}`} role="group" aria-label={t(`search.groups.${g.kind}`)} className="gs-g">
          <div className="gs-h" aria-hidden="true">
            {t(`search.groups.${g.kind}`)}
          </div>
          {g.hits.map((h) => {
            i++
            const n = i
            return option(
              n,
              h.id,
              `k-${g.kind}`,
              <>
                <Thumb hit={h} kind={g.kind} />
                <span className="gs-t">
                  <b>{h.title}</b>
                  <small>{sub(g, h)}</small>
                </span>
              </>,
              h.track ? { onContextMenu: (e: MouseEvent) => openTrackMenu(e, h.track!) } : {}
            )
          })}
        </div>
      ))}
      {!loading && !items.length && <span className="tmenu-note">{t('search.none', { q: query })}</span>}
      {loading && !items.length && <span className="tmenu-note">{t('search.searching')}</span>}
      {!inline && (
        <>
          <div className="tmenu-sep" role="separator" />
          {option(items.length, 'all', 'gs-all', <span className="gs-t">{t('search.all')}</span>)}
        </>
      )}
      <p className="gs-hint" aria-hidden="true">
        {t('search.hint')}
      </p>
    </>
  )

  return (
    <div
      ref={ref}
      id={id}
      className={`tmenu gsearch ${inline ? 'gs-inline' : ''}`}
      role="listbox"
      aria-label={t('search.results')}
      style={inline ? { maxHeight: MINI_RESULTS_MAX } : { left: box?.left, top: box?.top, width: box?.width, visibility: box ? 'visible' : 'hidden' }}
      // Clicks inside must not blur the input (that closes the dropdown)
      onPointerDown={(e) => e.preventDefault()}
      onMouseDown={(e) => e.preventDefault()}
    >
      {inline ? (
        <div ref={inner} className="gs-in">
          {body}
        </div>
      ) : (
        body
      )}
    </div>
  )
}

function Thumb({ hit, kind }: { hit: SearchHit; kind: SearchGroup['kind'] }) {
  const [broken, setBroken] = useState(false)
  return (
    <span className={`gs-th ${kind === 'artists' ? 'round' : ''}`} aria-hidden="true">
      {hit.coverId !== null && !broken ? (
        <img src={coverUrl(hit.coverId)} alt="" draggable={false} onError={() => setBroken(true)} />
      ) : (
        kind === 'artists' ? <Ico.Person /> : kind === 'playlists' ? <Ico.List /> : <Ico.Music />
      )}
    </span>
  )
}
