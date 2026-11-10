// Track context menu shared by every skin: right-click (or the Menu key / Shift+F10) on a track row
// calls openTrackMenu(); each Shell renders <TrackMenu /> once inside its root so the menu inherits
// the skin's font, colours and --v-* hooks (structure in views/views-base.css). In the 150 px mini
// player it is a native OS menu instead (it may leave the window; no "new list" field there).
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { create } from 'zustand'
import { api, native } from '../core/api'
import { t as tr, useT } from '../core/i18n'
import { setState, useStore } from '../core/store'
import type { Playlist, Track } from '../core/types'
import { leaveMini } from '../core/window'
import { popupMenu } from './nativeMenu'
import * as Ico from './shell/icons'
import './views/views-base.css'

export interface MenuItem {
  label: string
  run: () => void
  danger?: boolean
}

interface Req {
  x: number
  y: number
  track: Track
  /** "Reproducir" in this list's context (defaults to just this track) */
  onPlay?: () => void
  /** Extra actions for this list (e.g. "Quitar de la cola") */
  extra?: MenuItem[]
}

const useMenu = create<{ req: Req | null; toast: { text: string; n: number } | null }>(() => ({ req: null, toast: null }))

let toastTimer = 0
/** Short confirmation at the bottom of the window ("Añadida a la cola"). One line: keep it brief. */
export function toast(text: string, ms = 2200): void {
  clearTimeout(toastTimer)
  useMenu.setState((s) => ({ toast: { text, n: (s.toast?.n ?? 0) + 1 } }))
  toastTimer = window.setTimeout(() => useMenu.setState({ toast: null }), ms)
}

type At = { clientX: number; clientY: number; preventDefault?: () => void } | HTMLElement

/** Opens the menu at a mouse event, or under an element (keyboard). */
export function openTrackMenu(at: At, track: Track, opts: Pick<Req, 'onPlay' | 'extra'> = {}): void {
  if (native && useStore.getState().mini) {
    if (!(at instanceof HTMLElement)) at.preventDefault?.()
    return void nativeTrackMenu(track, opts, at instanceof HTMLElement ? at : undefined)
  }
  let x: number
  let y: number
  if (at instanceof HTMLElement) {
    const r = at.getBoundingClientRect()
    x = r.left + 24
    y = r.bottom - 4
  } else {
    at.preventDefault?.()
    x = at.clientX
    y = at.clientY
  }
  useMenu.setState({ req: { x, y, track, ...opts } })
}

/** Keyboard opener for focused rows: ContextMenu key or Shift+F10. */
export const isMenuKey = (e: ReactKeyboardEvent): boolean => e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')

/** "C:/Music/Rock/song.flac" → "C:/Music/Rock" (either separator) */
export const dirOf = (path: string): string => path.slice(0, Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\')))

const close = (): void => useMenu.setState({ req: null })

/** Runs a menu action, then a short confirmation (or the generic error). */
const run = (f: () => Promise<unknown> | void, done?: string) => (): void =>
  void Promise.resolve(f()).then(() => done && toast(done), () => toast(tr('err.unknown')))

/** The same actions as <Menu>, as a native menu (mini mode). */
async function nativeTrackMenu(track: Track, opts: Pick<Req, 'onPlay' | 'extra'>, at?: HTMLElement): Promise<void> {
  const [resume, lists] = await Promise.all([api.resumeGet(track.id).catch(() => null), api.playlists().catch(() => [] as Playlist[])])
  const ids = [track.id]
  const manual = lists.filter((p) => p.kind === 'manual')
  const more = [
    ...(resume !== null ? [{ text: tr('views.menu.fromStart'), run: run(() => api.resumeClear(track.id), tr('views.menu.fromStartDone')) }] : []),
    ...(track.external ? [] : [{ text: tr('views.menu.showFolder'), run: run(() => leaveMini().then(() => setState({ view: 'folders', folder: dirOf(track.path) }))) }])
  ]
  await popupMenu(
    [
      { text: track.artist ? `${track.title} · ${track.artist}` : track.title },
      { text: tr('views.menu.play'), run: run(opts.onPlay ?? (() => api.playTrack(track.id))) },
      { text: tr('views.menu.playNext'), run: run(() => api.queuePlayNext(ids), tr('views.menu.playNextDone')) },
      { text: tr('views.menu.addQueue'), run: run(() => api.queueAdd(ids), tr('views.menu.addQueueDone')) },
      manual.length
        ? { text: tr('views.menu.addList'), items: manual.map((p) => ({ text: p.name, run: run(() => api.playlistAdd(p.id, ids), tr('views.menu.addedTo', { name: p.name })) })) }
        : { text: tr('views.menu.addList') },
      ...(track.external ? [] : [{ text: tr('views.menu.radio'), run: run(() => api.radioStart(track.id), tr('views.menu.radioDone', { title: track.title })) }]),
      ...(more.length ? [null, ...more] : []),
      ...(opts.extra?.length ? [null, ...opts.extra.map((x) => ({ text: x.label, run: run(x.run) }))] : [])
    ],
    at
  )
}

export function TrackMenu() {
  const req = useMenu((s) => s.req)
  const tst = useMenu((s) => s.toast)
  return (
    <>
      {req && <Menu key={`${req.track.id}:${req.x}:${req.y}`} req={req} />}
      {tst && (
        <div className="tmenu-toast" role="status" key={tst.n}>
          {tst.text}
        </div>
      )}
    </>
  )
}

function focusItem(menu: HTMLElement | null, dir: 1 | -1 | 'first' | 'last'): void {
  if (!menu) return
  const items = [...menu.querySelectorAll<HTMLElement>(':scope > [role=menuitem], :scope > .tmenu-subwrap > [role=menuitem], :scope > form > input')]
  const i = items.indexOf(document.activeElement as HTMLElement)
  const n = dir === 'first' ? 0 : dir === 'last' ? items.length - 1 : (i + dir + items.length) % items.length
  items[n]?.focus()
}

function Menu({ req }: { req: Req }) {
  const t = useT()
  const ref = useRef<HTMLDivElement>(null)
  const subRef = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState({ x: req.x, y: req.y, ready: false })
  const [resume, setResume] = useState<number | null>(null)
  const [lists, setLists] = useState<Playlist[] | null>(null)
  const [sub, setSub] = useState<false | 'mouse' | 'key'>(false)
  const [naming, setNaming] = useState(false)
  const [subLeft, setSubLeft] = useState(false)
  const back = useRef<HTMLElement | null>(document.activeElement as HTMLElement | null)
  const { track } = req

  useEffect(() => {
    let alive = true
    api.resumeGet(track.id).then((r) => alive && setResume(r), () => {})
    api.playlists().then((l) => alive && setLists(l.filter((p) => p.kind === 'manual')), () => alive && setLists([]))
    return () => {
      alive = false
    }
  }, [track.id])

  // Keep inside the window, then focus the first item
  useLayoutEffect(() => {
    const el = ref.current!
    const r = el.getBoundingClientRect()
    const x = Math.max(6, Math.min(req.x, innerWidth - r.width - 6))
    const y = req.y + r.height > innerHeight - 6 ? Math.max(6, req.y - r.height) : req.y
    setPos({ x, y, ready: true })
    setSubLeft(x + r.width + 230 > innerWidth)
    if (!el.contains(document.activeElement)) focusItem(el, 'first')
  }, [req.x, req.y, resume])

  useEffect(() => {
    const away = (e: Event): void => {
      if (!ref.current?.contains(e.target as Node)) close()
    }
    const blur = (): void => close()
    window.addEventListener('pointerdown', away, true)
    window.addEventListener('wheel', away, true)
    window.addEventListener('resize', blur)
    window.addEventListener('blur', blur)
    return () => {
      window.removeEventListener('pointerdown', away, true)
      window.removeEventListener('wheel', away, true)
      window.removeEventListener('resize', blur)
      window.removeEventListener('blur', blur)
      back.current?.focus?.({ preventScroll: true })
    }
  }, [])

  useEffect(() => {
    if (sub === 'key') requestAnimationFrame(() => focusItem(subRef.current, 'first'))
  }, [sub])

  const act = (f: () => Promise<unknown> | void, done?: string) => () => {
    close()
    run(f, done)()
  }
  const ids = [track.id]
  const addTo = (p: Playlist) => act(() => api.playlistAdd(p.id, ids), t('views.menu.addedTo', { name: p.name }))

  const onKey = (e: ReactKeyboardEvent<HTMLDivElement>): void => {
    const inSub = !!subRef.current?.contains(document.activeElement)
    const menu = inSub ? subRef.current : ref.current
    if ((e.target as HTMLElement).tagName === 'INPUT' && e.key !== 'Escape' && e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') focusItem(menu, e.key === 'ArrowDown' ? 1 : -1)
    else if (e.key === 'Home' || e.key === 'End') focusItem(menu, e.key === 'Home' ? 'first' : 'last')
    else if (e.key === 'Escape' || (e.key === 'ArrowLeft' && inSub)) {
      if (inSub) {
        setSub(false)
        setNaming(false)
        ref.current?.querySelector<HTMLElement>('[aria-haspopup]')?.focus()
      } else close()
    } else if (e.key === 'ArrowRight' && (document.activeElement as HTMLElement)?.hasAttribute('aria-haspopup')) setSub('key')
    else if (e.key === 'Tab') close()
    else return
    e.preventDefault()
    e.stopPropagation()
  }

  const create = (name: string): void => {
    const n = name.trim()
    if (!n) return
    act(async () => {
      const p = await api.playlistCreate(n.slice(0, 200))
      await api.playlistAdd(p.id, ids)
    }, t('views.menu.addedTo', { name: n }))()
  }

  return (
    <div
      ref={ref}
      className="tmenu"
      role="menu"
      aria-label={t('views.menu.label', { title: track.title })}
      style={{ left: pos.x, top: pos.y, visibility: pos.ready ? 'visible' : 'hidden' }}
      onKeyDown={onKey}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="tmenu-h" aria-hidden="true">
        <b>{track.title}</b>
        <span>{track.artist}</span>
      </div>
      <button role="menuitem" tabIndex={-1} className="tmenu-i" onClick={act(req.onPlay ?? (() => api.playTrack(track.id)))} onPointerEnter={() => setSub(false)}>
        {t('views.menu.play')}
      </button>
      <button role="menuitem" tabIndex={-1} className="tmenu-i" onClick={act(() => api.queuePlayNext(ids), t('views.menu.playNextDone'))} onPointerEnter={() => setSub(false)}>
        {t('views.menu.playNext')}
      </button>
      <button role="menuitem" tabIndex={-1} className="tmenu-i" onClick={act(() => api.queueAdd(ids), t('views.menu.addQueueDone'))} onPointerEnter={() => setSub(false)}>
        {t('views.menu.addQueue')}
      </button>
      <div className="tmenu-subwrap" onPointerEnter={() => setSub((s) => s || 'mouse')} onPointerLeave={() => !naming && setSub(false)}>
        <button
          role="menuitem"
          tabIndex={-1}
          className={`tmenu-i has-sub ${sub ? 'open' : ''}`}
          aria-haspopup="menu"
          aria-expanded={!!sub}
          onClick={() => setSub('key')}
        >
          {t('views.menu.addList')}
          <span className="tmenu-arrow" aria-hidden="true">
            <Ico.ChevronRight />
          </span>
        </button>
        {sub && (
          <div ref={subRef} className={`tmenu tmenu-sub ${subLeft ? 'left' : ''}`} role="menu" aria-label={t('views.menu.addList')}>
            {lists === null ? (
              <span className="tmenu-note">{t('app.loading')}</span>
            ) : (
              lists.map((p) => (
                <button key={p.id} role="menuitem" tabIndex={-1} className="tmenu-i" onClick={addTo(p)}>
                  <span className="tmenu-ell">{p.name}</span>
                </button>
              ))
            )}
            {lists && lists.length > 0 && <div className="tmenu-sep" role="separator" />}
            {naming ? (
              <form
                onSubmit={(e) => {
                  e.preventDefault()
                  create(new FormData(e.currentTarget).get('name') as string)
                }}
              >
                <input name="name" className="tmenu-input" autoFocus maxLength={200} placeholder={t('views.playlists.namePlaceholder')} aria-label={t('views.playlists.name')} />
              </form>
            ) : (
              <button role="menuitem" tabIndex={-1} className="tmenu-i" onClick={() => setNaming(true)}>
                {t('views.menu.newList')}
              </button>
            )}
          </div>
        )}
      </div>
      {!track.external && (
        <button role="menuitem" tabIndex={-1} className="tmenu-i" onClick={act(() => api.radioStart(track.id), t('views.menu.radioDone', { title: track.title }))} onPointerEnter={() => setSub(false)}>
          {t('views.menu.radio')}
        </button>
      )}
      {(resume !== null || !track.external) && <div className="tmenu-sep" role="separator" />}
      {resume !== null && (
        <button role="menuitem" tabIndex={-1} className="tmenu-i" onClick={act(() => api.resumeClear(track.id), t('views.menu.fromStartDone'))} onPointerEnter={() => setSub(false)}>
          {t('views.menu.fromStart')}
        </button>
      )}
      {!track.external && (
        <button
          role="menuitem"
          tabIndex={-1}
          className="tmenu-i"
          onPointerEnter={() => setSub(false)}
          onClick={act(() => setState({ view: 'folders', folder: dirOf(track.path) }))}
        >
          {t('views.menu.showFolder')}
        </button>
      )}
      {req.extra && req.extra.length > 0 && (
        <>
          <div className="tmenu-sep" role="separator" />
          {req.extra.map((x) => (
            <button key={x.label} role="menuitem" tabIndex={-1} className={`tmenu-i ${x.danger ? 'danger' : ''}`} onClick={act(x.run)} onPointerEnter={() => setSub(false)}>
              {x.label}
            </button>
          ))}
        </>
      )}
    </div>
  )
}
