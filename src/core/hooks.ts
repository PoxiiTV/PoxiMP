import { useCallback, useEffect, useRef, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, native, on } from './api'
import { t } from './i18n'
import { setState, useStore } from './store'
import type { QualityReport, QualitySettings, QualityState, SortKey, Track } from './types'

const PAGE = 200
const inflight = new Set<string>()

// ponytail: backend errors are swallowed until there is a toast system to show them
const run = (p: Promise<unknown>): void => void p.catch(() => {})

/** Wires backend events into the store and loads the first library page. Call once. */
export function boot(): void {
  on('player:state', (player) => setState({ player }))
  on('player:position', (p) => setState({ pos: { ...p, at: performance.now() } }))
  on('player:track', ({ track }) => setState({ track }))
  on('player:output', (output) => setState({ output }))
  on('quality:update', (u) => {
    setState((s) => ({ quality: { ...s.quality, [u.trackId]: u } }))
    if (u.verdict !== 'analyzing') refreshTotals()
  })
  on('library:progress', (scan) => {
    const done = scan.done >= scan.total
    setState({ scan: done ? null : scan })
    if (done) {
      reloadLibrary()
      refreshTotals()
    }
  })
  run(api.sync().then(({ state, track, position }) => setState({ player: state, track, pos: { ...position, at: performance.now() } })))
  run(api.outputInfo().then((output) => setState({ output })))
  run(api.qualitySettings().then((qset) => setState({ qset })))
  reloadLibrary()
  refreshTotals()
}

let totalsTimer = 0
/** Whole-library counts for the status bar; quality updates come in bursts, so at most every 3 s. */
function refreshTotals(): void {
  if (totalsTimer) return
  totalsTimer = window.setTimeout(() => (totalsTimer = 0), 3000)
  run(api.page({ offset: 0, limit: 1, sort: 'artist', filter: '' }).then(({ total, duration, counts }) => setState({ totals: { total, duration, counts } })))
}

/** The detector switch (Ajustes → Calidad real). Off pauses the analysis in the backend. */
export function setQualitySettings(qset: QualitySettings): void {
  const before = useStore.getState().qset
  setState({ qset })
  api.setQualitySettings(qset).then((q) => setState({ qset: q }), () => setState({ qset: before }))
}

// --- library (paged) ---

/** Drops loaded rows and starts again from page 0 (new filter/sort/artist or after a scan). */
export function reloadLibrary(patch: { filter?: string; sort?: SortKey; artist?: number | null } = {}): void {
  const lib = useStore.getState().library
  setState({ library: { ...lib, ...patch, rows: [], gen: lib.gen + 1 } })
  loadPage(0)
}

function loadPage(p: number): void {
  const { gen, sort, filter, artist } = useStore.getState().library
  const key = `${gen}:${p}`
  if (inflight.has(key)) return
  inflight.add(key)
  api
    .page({ offset: p * PAGE, limit: PAGE, sort, filter, artistId: artist })
    .then((res) => {
      const lib = useStore.getState().library
      if (lib.gen !== gen) return
      const rows = lib.rows.slice()
      rows.length = res.total
      res.tracks.forEach((t, i) => (rows[p * PAGE + i] = t))
      setState({ library: { ...lib, total: res.total, duration: res.duration, counts: res.counts, rows, ready: true } })
    })
    .catch(() => inflight.delete(key))
}

/** Makes sure rows start..end (inclusive) are loaded or on their way. */
export function ensureRange(start: number, end: number): void {
  const { rows } = useStore.getState().library
  for (let p = Math.floor(start / PAGE); p <= Math.floor(end / PAGE); p++) if (!rows[p * PAGE]) loadPage(p)
}

/** Asks for a folder (OS dialog), adds it and rescans. Resolves to the folder list, or null if cancelled. */
export async function addFolder(): Promise<string[] | null> {
  const path = native ? await open({ directory: true, title: t('settings.pickFolder') }) : 'C:/Music/Demo'
  if (typeof path !== 'string') return null
  await api.addFolder(path)
  run(api.scan())
  return api.folders()
}

/** Takes a folder out of the library (files stay on disk). Resolves to the folder list. */
export async function removeFolder(path: string): Promise<string[]> {
  await api.removeFolder(path)
  reloadLibrary()
  clearTimeout(totalsTimer)
  totalsTimer = 0
  refreshTotals()
  return api.folders()
}

export function useLibrary() {
  return { ...useStore((s) => s.library), ensureRange, reloadLibrary }
}

// --- player ---

/** Where playback is now (interpolated between the 4 Hz position events). */
export function nowSecs(): number {
  const { pos, player: p } = useStore.getState()
  const secs = p.status === 'playing' ? pos.secs + (Math.max(0, performance.now() - pos.at) / 1000) * (pos.speed ?? 1) : pos.secs
  return Math.min(secs, pos.duration || secs)
}

export const player = {
  toggle: () => run(api.toggle()),
  next: () => run(api.next()),
  previous: () => run(api.previous()),
  stop: () => run(api.stop()),
  /** A-B button: mark A here → mark B here (loop) → off. */
  cycleAb() {
    const ab = useStore.getState().player.ab
    const at = nowSecs()
    if (!ab) run(api.abSet(at, null))
    else if (ab.b === null && at - ab.a >= 0.5) run(api.abSet(ab.a, at))
    // Went back before A: A moves here
    else if (ab.b === null && at < ab.a) run(api.abSet(at, null))
    else if (ab.b !== null) run(api.abSet(null, null))
  },
  /** Sleep timer: minutes, 'track' = when this one ends, null = off. */
  sleep: (v: number | 'track' | null) => run(api.sleepSet(typeof v === 'number' ? v : null, v === 'track')),
  seek(secs: number) {
    setState((s) => ({ pos: { ...s.pos, secs, at: performance.now() } }))
    run(api.seek(secs))
  },
  setVolume(v: number) {
    // Bit-perfect: fixed at 100 % (the backend would refuse it anyway).
    if (useStore.getState().player.volumeLocked) return
    setState((s) => ({ player: { ...s.player, volume: v } }))
    run(api.setVolume(v))
  },
  toggleShuffle: () => run(api.setShuffle(!useStore.getState().player.shuffle)),
  cycleRepeat() {
    const r = useStore.getState().player.repeat
    run(api.setRepeat(r === 'off' ? 'all' : r === 'all' ? 'one' : 'off'))
  },
  /** Plays the library view (sort + filter, all of it, server-side) starting at that row. */
  playRow(index: number) {
    const { rows, sort, filter, artist } = useStore.getState().library
    const target = rows[index]
    if (target) run(api.playView({ sort, filter, artistId: artist }, target.id))
  }
}

/** Why the volume cannot move (bit-perfect output fixes it at 100 %), or undefined. */
export function useVolumeLock(): string | undefined {
  useStore((s) => s.config?.lang)
  return useStore((s) => s.player.volumeLocked) ? t('player.volumeLocked') : undefined
}

export function usePlayer() {
  return { state: useStore((s) => s.player), track: useStore((s) => s.track), ...player }
}

/** Position interpolated every frame from the 4 Hz backend events. Use in small components only. */
export function usePosition(): { secs: number; duration: number } {
  const pos = useStore((s) => s.pos)
  const playing = useStore((s) => s.player.status === 'playing')
  const [now, setNow] = useState(0)
  useEffect(() => {
    if (!playing) return
    let id = requestAnimationFrame(function frame(t) {
      setNow(t)
      id = requestAnimationFrame(frame)
    })
    return () => cancelAnimationFrame(id)
  }, [playing])
  const secs = playing ? pos.secs + (Math.max(0, now - pos.at) / 1000) * (pos.speed ?? 1) : pos.secs
  return { secs: Math.min(secs, pos.duration), duration: pos.duration }
}

/** Seekbar waveform of a track (peaks then RMS, see waveform_get); null until the backend has it. */
export function useWaveform(id: number | null): Uint8Array | null {
  const [wave, setWave] = useState<{ id: number; data: Uint8Array } | null>(null)
  useEffect(() => {
    if (id === null) return
    let alive = true
    const load = (): void =>
      run(
        api.waveform(id).then((b) => {
          if (alive && b.byteLength) setWave({ id, data: new Uint8Array(b) })
        })
      )
    load()
    const off = on('waveform:ready', (e) => e.trackId === id && load())
    return () => {
      alive = false
      off()
    }
  }, [id])
  return wave?.id === id ? wave.data : null
}

/** Pulls a spectrum frame per animation frame while playing (one request in flight at most).
 *  `draw` gets the bytes (layout in types.ts) or null when paused/stopped, so it can decay. */
export function useSpectrum(bands: number, draw: (frame: Uint8Array | null) => void): void {
  const playing = useStore((s) => s.player.status === 'playing')
  const ref = useRef(draw)
  useEffect(() => {
    ref.current = draw
  })
  useEffect(() => {
    let alive = true
    let busy = false
    let id = requestAnimationFrame(function frame() {
      if (!playing) ref.current(null)
      else if (!busy) {
        busy = true
        api
          .vizFrame(bands)
          .then((b) => alive && ref.current(new Uint8Array(b)))
          .catch(() => {})
          .finally(() => (busy = false))
      }
      id = requestAnimationFrame(frame)
    })
    return () => {
      alive = false
      cancelAnimationFrame(id)
    }
  }, [bands, playing])
}

export interface Quality {
  state: QualityState
  report: QualityReport | null
  progress?: number
}

/** A track's quality, live updates included; fetches the report if the row came without it. */
export function useQuality(t: Track | null): Quality | null {
  const live = useStore((s) => (t ? s.quality[t.id] : undefined))
  const [fetched, setFetched] = useState<{ id: number; report: QualityReport | null } | null>(null)
  const state = live?.verdict ?? t?.quality
  const report = live ? live.report : (t?.report ?? null)
  const missing = !!t && !report && state !== 'analyzing' && state !== 'pending'
  useEffect(() => {
    if (!missing || !t) return
    let alive = true
    run(api.qualityReport(t.id).then((r) => alive && setFetched({ id: t.id, report: r })))
    return () => {
      alive = false
    }
  }, [missing, t])
  if (!t || !state) return null
  return { state, report: report ?? (fetched?.id === t.id ? fetched.report : null), progress: live ? live.progress : t.progress }
}

// --- generic paged track lists (folder view, queue, playlists) ---

export interface Paged<R> {
  total: number
  /** Sparse like Library.rows; after reload() the old rows stay until the new ones arrive (no flicker) */
  rows: (Track | undefined)[]
  ready: boolean
  /** Last full response (duration, index…) */
  last: R | null
  ensureRange: (start: number, end: number) => void
  /** Refetch what is loaded (after an edit or a backend event) */
  reload: () => void
  /** Optimistic local edit of the loaded rows (reorder / remove before the backend answers) */
  edit: (f: (rows: (Track | undefined)[]) => (Track | undefined)[]) => void
}

/** Pages through any `{ total, tracks }` source. `key` change = a different list: start again. */
export function usePaged<R extends { total: number; tracks: Track[] }>(fetch: (offset: number, limit: number) => Promise<R>, key: string): Paged<R> {
  const [st, setSt] = useState({ key, gen: 0, total: 0, rows: [] as (Track | undefined)[], ready: false, last: null as R | null })
  const ref = useRef(st)
  ref.current = st
  const fetchRef = useRef(fetch)
  fetchRef.current = fetch
  const inflight = useRef(new Set<string>())

  const load = useCallback((p: number, gen: number) => {
    const k = `${gen}:${p}`
    if (inflight.current.has(k)) return
    inflight.current.add(k)
    fetchRef
      .current(p * PAGE, PAGE)
      .then((res) =>
        setSt((s) => {
          if (s.gen !== gen) return s
          const rows = s.rows.slice()
          rows.length = res.total
          res.tracks.forEach((t, i) => (rows[p * PAGE + i] = t))
          return { ...s, total: res.total, rows, ready: true, last: res }
        })
      )
      .catch(() => {
        inflight.current.delete(k)
        setSt((s) => (s.gen === gen ? { ...s, ready: true } : s))
      })
  }, [])

  const reload = useCallback(() => {
    const s = ref.current
    const gen = s.gen + 1
    setSt({ ...s, gen })
    ref.current = { ...s, gen }
    const pages = Math.max(1, Math.ceil(s.rows.length / PAGE))
    for (let p = 0; p < pages; p++) if (p === 0 || s.rows[p * PAGE]) load(p, gen)
  }, [load])

  useEffect(() => {
    const gen = ref.current.gen + 1
    const next = { key, gen, total: 0, rows: [], ready: false, last: null }
    setSt(next)
    ref.current = next
    load(0, gen)
  }, [key, load])

  const ensureRange = useCallback(
    (start: number, end: number) => {
      const { rows, gen } = ref.current
      for (let p = Math.floor(start / PAGE); p <= Math.floor(end / PAGE); p++) if (!rows[p * PAGE]) load(p, gen)
    },
    [load]
  )

  const edit = useCallback((f: (rows: (Track | undefined)[]) => (Track | undefined)[]) => setSt((s) => {
    const rows = f(s.rows.slice())
    return { ...s, rows, total: s.total + rows.length - s.rows.length }
  }), [])

  // A key change renders once with the old rows before the effect runs: hide them
  const fresh = st.key === key
  return { total: fresh ? st.total : 0, rows: fresh ? st.rows : [], ready: fresh && st.ready, last: fresh ? st.last : null, ensureRange, reload, edit }
}
