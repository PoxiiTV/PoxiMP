// Mini mode = the same window shrunk to the player panel (like AIMP). The layout collapses to the
// panel whenever the window is shorter than COLLAPSE_BELOW, so dragging the edge also works both
// ways; the button / Ctrl+M resizes the window to the panel's height and back to the last full size.
// Sizes and the mode live in localStorage and are restored before the window is first shown.
import { useEffect } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { currentMonitor, getCurrentWindow, LogicalSize, PhysicalPosition } from '@tauri-apps/api/window'
import { api, native } from './api'
import { setState, useStore, ZOOMS } from './store'
import { isMac } from '../ui/WindowControls'

/** Shorter than this, only the player panel shows. */
export const COLLAPSE_BELOW = 400
/** The mini player (it has no title bar), docs/bocetos/08-mini.html. Same as tauri.conf.json minHeight. */
const MINI_H = 150
const FULL_DEFAULT = { w: 1280, h: 800 }
const KEY = 'poximp.win'

interface Saved {
  mini: boolean
  fullW: number
  fullH: number
  miniW: number
}

function saved(): Saved {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) ?? '') as Saved
    if (typeof s.fullW === 'number' && typeof s.fullH === 'number') return s
  } catch {
    /* first run */
  }
  return { mini: false, fullW: FULL_DEFAULT.w, fullH: FULL_DEFAULT.h, miniW: FULL_DEFAULT.w }
}
const save = (patch: Partial<Saved>): void => localStorage.setItem(KEY, JSON.stringify({ ...saved(), ...patch }))

/** Outside Tauri the window cannot be resized: the button just collapses the layout (preview). */
let forced = false
/** Mini's search results have grown the window (growMini): its height says nothing about the mode. */
let grown = false
const collapsed = (): boolean => forced || grown || innerHeight < COLLAPSE_BELOW
/** toggleMini is under way: the results do not grow the window meanwhile */
let switching = false

// ---------- interface scale ----------

/** Webview zoom: page (CSS) px are this many window (logical) px. Every size here is in CSS px. */
let zoom = 1
export const uiZoom = (): number => zoom
/** CSS px to a window size */
const size = (w: number, h: number): LogicalSize => new LogicalSize(Math.round(w * zoom), Math.round(h * zoom))

/** Applies Ajustes > Escala; the window keeps its size, so everything inside grows or shrinks.
 *  The minimum size follows, so the mini player always gets its 150 px. */
export async function applyZoom(z: number): Promise<void> {
  zoom = ZOOMS.includes(z) ? z : 1
  if (!native) return
  await getCurrentWebview().setZoom(zoom)
  await getCurrentWindow().setMinSize(size(640, MINI_H))
}

async function resize(w: number, h: number): Promise<void> {
  const win = getCurrentWindow()
  if (await win.isMaximized()) await win.unmaximize()
  await win.setSize(size(w, h))
}

/** Button and Ctrl/Cmd+M. */
export async function toggleMini(): Promise<void> {
  switching = true
  try {
    if (collapsed()) {
      // Search results open: back to the bare panel (and its place) first
      await growMini(0)
      forced = false
      const s = saved()
      save({ mini: false })
      if (native) await resize(s.fullW, Math.max(s.fullH, COLLAPSE_BELOW))
    } else {
      if (useStore.getState().view === 'settings') setState({ view: 'library' })
      save({ mini: true, fullW: innerWidth, fullH: innerHeight, miniW: innerWidth })
      if (native) await resize(innerWidth, MINI_H)
      else forced = true
    }
    setState({ mini: collapsed() })
  } finally {
    switching = false
  }
}

// ---------- mini search results (docs/bocetos/11-buscador-mini.html) ----------

/** Physical px the window went up so the results fit above the taskbar */
let lifted = 0
let want = 0
let running: Promise<void> | null = null

/** Grows the mini window downward by `extra` px for the search results (0 = back to MINI_H and
 *  to where it was). Calls coalesce: only the last height asked for is applied. Never saved. */
export function growMini(extra: number): Promise<void> {
  want = Math.max(0, Math.round(extra))
  running ??= (async () => {
    try {
      for (let done = -1; done !== want; ) await applyGrow((done = want))
    } catch {
      // A call was refused: when closing, at least let the height speak for the mode again
      if (!want) grown = false
    } finally {
      running = null
    }
  })()
  return running
}

async function applyGrow(extra: number): Promise<void> {
  if (!native) return
  const win = getCurrentWindow()
  if (!extra) {
    if (!grown) return
    await win.setSize(size(innerWidth, MINI_H))
    if (lifted) {
      const p = await win.outerPosition()
      await win.setPosition(new PhysicalPosition(p.x, p.y + lifted))
      lifted = 0
    }
    // Keep the layout in mini until the shrink has landed (else it would flash the full layout)
    await below(COLLAPSE_BELOW)
    grown = false
    return
  }
  if (switching || !useStore.getState().mini || (!grown && innerHeight >= COLLAPSE_BELOW)) return
  grown = true
  const [pos, outer, scale, mon] = await Promise.all([win.outerPosition(), win.outerSize(), win.scaleFactor(), currentMonitor()])
  const outerH = outer.height + Math.round((MINI_H + extra - innerHeight) * scale * zoom)
  const top = pos.y + lifted
  let y = top
  // Past the bottom of the work area: go up as much as needed (never above its top)
  if (mon) {
    const { position: wp, size: ws } = mon.workArea
    y = Math.min(top, Math.max(wp.y, wp.y + ws.height - outerH))
  }
  lifted = top - y
  const move = (): Promise<void> => (y === pos.y ? Promise.resolve() : win.setPosition(new PhysicalPosition(pos.x, y)))
  if (y < pos.y) await move()
  await win.setSize(size(innerWidth, MINI_H + extra))
  if (y > pos.y) await move()
}

/** Resolves once the window is shorter than `h` (or after half a second). */
function below(h: number): Promise<void> {
  return new Promise((done) => {
    const check = (): void => {
      if (innerHeight < h) finish()
    }
    const finish = (): void => {
      clearTimeout(timer)
      removeEventListener('resize', check)
      done()
    }
    const timer = setTimeout(finish, 500)
    addEventListener('resize', check)
    check()
  })
}

/** Back to the full window if it is in mini mode (Ajustes needs the room). */
export async function leaveMini(): Promise<void> {
  if (collapsed()) await toggleMini()
}

/** Last size and mode, before the window is shown (main.tsx). */
export async function restoreWindow(): Promise<void> {
  if (!native) return
  await applyZoom(useStore.getState().prefs.zoom).catch(() => {})
  const s = saved()
  await (s.mini ? resize(s.miniW, MINI_H) : resize(s.fullW, Math.max(s.fullH, COLLAPSE_BELOW))).catch(() => {})
  setState({ mini: collapsed() })
}

/** macOS: the native traffic lights have no title bar in mini, so they go over the cover (its left
 *  edge), in line with the search pill (the room above the cover: shell.css); full window: AppKit's place. */
async function placeLights(mini: boolean): Promise<void> {
  if (!native || !isMac) return
  const cov = mini ? document.querySelector('.app.mini .left > .cov') : null
  const bar = document.querySelector('.app.mini .mbar')
  if (!cov || !bar) return api.windowLights(null)
  const c = cov.getBoundingClientRect()
  const b = bar.getBoundingClientRect()
  await api.windowLights([c.left * zoom, (b.top + b.height / 2) * zoom])
}

/** Keeps store.mini in step with the height, remembers sizes, and applies "always on top". */
export function useWindowMode(): void {
  const mini = useStore((s) => s.mini)
  const onTop = useStore((s) => (s.mini ? s.prefs.miniOnTop : s.prefs.onTop))
  const z = useStore((s) => s.prefs.zoom)
  useEffect(() => {
    if (z !== zoom) void applyZoom(z).catch(() => {})
  }, [z])
  useEffect(() => {
    let timer = 0
    const onResize = (): void => {
      setState({ mini: collapsed() })
      clearTimeout(timer)
      timer = window.setTimeout(async () => {
        if (!native) return
        const max = await getCurrentWindow().isMaximized().catch(() => false)
        if (collapsed()) save({ mini: true, miniW: innerWidth })
        else if (!max) save({ mini: false, fullW: innerWidth, fullH: innerHeight })
      }, 400)
    }
    onResize()
    window.addEventListener('resize', onResize)
    return () => {
      clearTimeout(timer)
      window.removeEventListener('resize', onResize)
    }
  }, [])
  useEffect(() => {
    if (native) void getCurrentWindow().setAlwaysOnTop(onTop).catch(() => {})
  }, [onTop, mini])
  const skin = useStore((s) => s.skin)
  useEffect(() => {
    void placeLights(mini).catch(() => {})
  }, [mini, z, skin])
}
