import { create } from 'zustand'
import type { Lang } from './i18n'
import type { LibraryPage, OutputInfo, PlayerState, QualitySettings, QualityUpdate, ScanProgress, SortKey, Track } from './types'

export interface Config {
  lang?: Lang
}

export type Variant = 'dark' | 'light'
export interface SkinChoice {
  id: string
  variant: Variant
}

export interface Library extends Omit<LibraryPage, 'tracks'> {
  /** Sparse: index = row in the current sort/filter; undefined = page not loaded yet */
  rows: (Track | undefined)[]
  sort: SortKey
  filter: string
  /** Artist column pick (Artist.id), null = everyone */
  artist: number | null
  /** Bumped on every reset so late page responses of an old query are dropped */
  gen: number
  /** A page has arrived at least once (tells "empty library" from "not loaded yet") */
  ready: boolean
}

/** Interface preferences kept in this machine's localStorage (the sound ones live in Rust). */
export interface Prefs {
  /** The skin's own fonts, or a neutral one on every skin */
  font: 'skin' | 'neutral'
  /** Mini mode keeps the window above the others */
  miniOnTop: boolean
  /** "Always on top" button, outside mini mode */
  onTop: boolean
  /** null = follow the system */
  lang: Lang | null
  /** Interface scale (webview zoom), one of ZOOMS */
  zoom: number
}

/** Ajustes > Apariencia > Escala */
export const ZOOMS = [0.8, 0.9, 1, 1.1, 1.25, 1.5]

interface State {
  config: Config | null
  player: PlayerState
  track: Track | null
  /** Last position from the backend and when it arrived (performance.now()), for interpolation */
  /** `speed`: source seconds per second (Efectos), the clock between events */
  pos: { secs: number; duration: number; speed?: number; at: number }
  library: Library
  /** Whole library (no filter): status bar */
  totals: Omit<LibraryPage, 'tracks'> | null
  /** Search box text; becomes library.filter after a short debounce */
  search: string
  /** Live results from "quality:update", newer than what the rows carry */
  quality: Record<number, QualityUpdate>
  qset: QualitySettings
  selected: Track | null
  skin: SkinChoice
  prefs: Prefs
  /** Mini mode chosen (button / Ctrl+M); a window dragged short also shows only the player */
  mini: boolean
  view: View
  /** Ajustes section open */
  section: Section
  /** Carpetas: selected FolderNode.path (null = none yet) */
  folder: string | null
  /** Listas: open playlist (null = the index) */
  playlist: number | null
  /** 'browse' view: the album or artist opened from the search bar */
  browse: Browse | null
  scan: ScanProgress | null
  output: OutputInfo | null
}

export type View = 'library' | 'folders' | 'queue' | 'lyrics' | 'playlists' | 'radio' | 'stats' | 'wrapped' | 'browse' | 'settings'
/** Tabs, in this order on every skin (Wrapped opens from Estadísticas, an album / artist page from
 *  the search bar, Ajustes from its button in the title bar). */
export const NAV: View[] = ['queue', 'library', 'folders', 'playlists', 'radio', 'lyrics', 'stats']

export const SECTIONS = ['general', 'appearance', 'playback', 'output', 'eq', 'fx', 'library', 'quality', 'lyrics', 'remote', 'keys', 'import', 'about'] as const
export type Section = (typeof SECTIONS)[number]

export interface Browse {
  kind: 'album' | 'artist'
  /** Album key / artist id (SearchHit.id) */
  id: string
  title: string
}

export const SKIN_KEY = 'poximp.skin'
const PREFS_KEY = 'poximp.ui'

function load<T>(key: string, ok: (v: T) => boolean, fallback: T): T {
  try {
    const v = JSON.parse(localStorage.getItem(key) ?? '') as T
    if (ok(v)) return v
  } catch {
    /* nothing saved yet */
  }
  return fallback
}

export const savedSkin = (): SkinChoice =>
  load<SkinChoice>(SKIN_KEY, (s) => typeof s?.id === 'string' && (s.variant === 'dark' || s.variant === 'light'), { id: 'caraa', variant: 'dark' })

const DEFAULT_PREFS: Prefs = { font: 'skin', miniOnTop: true, onTop: false, lang: null, zoom: 1 }
const savedPrefs = (): Prefs => ({ ...DEFAULT_PREFS, ...load<Partial<Prefs>>(PREFS_KEY, (p) => typeof p === 'object' && p !== null, {}) })

const prefs = savedPrefs()

export const useStore = create<State>(() => ({
  config: { lang: prefs.lang ?? undefined },
  player: { status: 'stopped', volume: 0.8, volumeLocked: false, shuffle: false, repeat: 'off', index: -1, queueLength: 0, next: null, ab: null, sleep: null },
  track: null,
  pos: { secs: 0, duration: 0, at: 0 },
  library: { total: 0, duration: 0, counts: { ok: 0, doubt: 0, pending: 0 }, rows: [], sort: 'artist', filter: '', artist: null, gen: 0, ready: false },
  totals: null,
  search: '',
  quality: {},
  qset: { analyze: true, showNoSigns: true },
  selected: null,
  skin: savedSkin(),
  prefs,
  mini: false,
  view: 'library',
  section: 'general',
  folder: null,
  playlist: null,
  browse: null,
  scan: null,
  output: null
}))

export const setState = useStore.setState

export const go = (view: View): void => setState({ view })

export function setSkin(skin: SkinChoice): void {
  localStorage.setItem(SKIN_KEY, JSON.stringify(skin))
  setState({ skin })
}

export function setPrefs(patch: Partial<Prefs>): void {
  const p = { ...useStore.getState().prefs, ...patch }
  localStorage.setItem(PREFS_KEY, JSON.stringify(p))
  setState({ prefs: p, ...('lang' in patch ? { config: { lang: p.lang ?? undefined } } : {}) })
}
