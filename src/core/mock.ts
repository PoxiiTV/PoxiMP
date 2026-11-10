// In-memory backend for previewing the UI in a plain browser (vite without Tauri).
// Same commands and events as the real one (see types.ts); the demo data is the mockups' list.
import type {
  AutoEqEntry,
  EqBand,
  EqPreset,
  EqSettings,
  EqState,
  FxSettings,
  FolderNode,
  ImportReport,
  LibraryPage,
  Lyrics,
  Mix,
  NormMode,
  OutputDevice,
  OutputInfo,
  OutputSettings,
  PageQuery,
  PlayerState,
  Playlist,
  PlaylistPage,
  QualityReport,
  QualitySettings,
  QualityState,
  RemoteInfo,
  RemotePhone,
  RepeatMode,
  SearchGroup,
  ShortcutInfo,
  SmartNode,
  SmartRules,
  Stats,
  StatsRange,
  TopItem,
  TopTrack,
  Track,
  Wrapped
} from './types'

type Handler = (payload: unknown) => void
const handlers = new Map<string, Set<Handler>>()

function emit(event: string, payload: unknown): void {
  handlers.get(event)?.forEach((h) => h(payload))
}

export function listen(event: string, h: Handler): () => void {
  if (!handlers.has(event)) handlers.set(event, new Set())
  handlers.get(event)!.add(h)
  return () => void handlers.get(event)?.delete(h)
}

const rep = (r: Partial<QualityReport> & Pick<QualityReport, 'verdict'>): QualityReport => ({
  confidence: 0.95,
  cutoffHz: null,
  bandwidthHz: 22000,
  nyquistHz: 22050,
  estKbps: null,
  origin: null,
  realBits: null,
  realRate: null,
  ...r
})

let nextId = 1
function track(
  title: string,
  artist: string,
  album: string,
  dur: string,
  fmt: [format: string, bitrate: number, rate: number, bits: number | null],
  quality: QualityState,
  report: QualityReport | null,
  progress?: number
): Track {
  const [m, s] = dur.split(':').map(Number)
  const [format, bitrate, sampleRate, bitDepth] = fmt
  return {
    id: nextId++,
    path: `C:/Music/${artist}/${title}.${format.toLowerCase()}`,
    title,
    artist,
    album,
    duration: m * 60 + s,
    format,
    bitrate,
    sampleRate,
    bitDepth,
    lossless: bitDepth !== null,
    quality,
    report,
    progress,
    // Filled in from the play history below
    year: null,
    genre: '',
    playCount: 0,
    lastPlayedAt: null,
    replayGain: bitDepth !== null ? -7.4 + (nextId % 5) : null
  }
}

const tracks: Track[] = [
  track('AURORA', 'Mora', 'Estrella', '3:42', ['FLAC', 912, 44100, 16], 'real', rep({ verdict: 'real', confidence: 0.99 })),
  track('DROGA', 'Mora, C. Tangana', 'Estrella', '3:15', ['MP3', 320, 44100, null], 'fake',
    rep({ verdict: 'fake', confidence: 0.94, cutoffHz: 16000, bandwidthHz: 16000, estKbps: 128 })),
  track('CHUOS', 'Juseph, Quevedo', 'CHUOS', '2:58', ['WAV', 1411, 44100, 16], 'lossyOrigin',
    rep({ verdict: 'lossyOrigin', confidence: 0.89, cutoffHz: 19000, bandwidthHz: 19000, estKbps: 256, origin: 'mp3' })),
  track('tu contactO', 'Raul Clyde, Sergi El Combo, Xiyo, Fernandezz', 'tu contactO', '3:31', ['MP3', 320, 44100, null], 'noSigns',
    rep({ verdict: 'noSigns', confidence: 0.96, cutoffHz: 20000, bandwidthHz: 20000 })),
  track('Da Me', 'Bad Gyal', 'Da Me', '2:47', ['MP3', 256, 44100, null], 'analyzing', null, 0.62),
  track('Maniatika', 'Alex Brown', 'Maniatika', '3:05', ['FLAC', 2420, 96000, 24], 'fakeHiRes',
    rep({ verdict: 'fakeHiRes', confidence: 0.97, cutoffHz: 22000, bandwidthHz: 22000, nyquistHz: 48000, realBits: 16, realRate: 44100 })),
  track('Mandanga Style', 'Amador Rivas', 'Mandanga Style', '3:20', ['WAV', 1411, 44100, 16], 'real', rep({ verdict: 'real', confidence: 0.98 })),
  track('Plakito', 'Llandel Veguilla', 'Plakito', '2:39', ['MP3', 192, 44100, null], 'noSigns',
    rep({ verdict: 'noSigns', confidence: 0.93, cutoffHz: 18600, bandwidthHz: 18600 })),
  track('Soldado Y Profeta', 'Anuel AA', 'Soldado Y Profeta', '4:12', ['MP3', 320, 44100, null], 'fake',
    rep({ verdict: 'fake', confidence: 0.91, cutoffHz: 18500, bandwidthHz: 18500, estKbps: 192 })),
  track('Pendiente', 'Mora', 'Estrella', '3:09', ['M4A', 256, 44100, null], 'pending', null),
  // Long mix: remembers its position (resume_get)
  track('Sesion DJ Hummer FEBRERO 2025', 'DJ Hummer', 'Sesiones', '24:24', ['MP3', 320, 44100, null], 'noSigns',
    rep({ verdict: 'noSigns', confidence: 0.9, cutoffHz: 20000, bandwidthHz: 20000 })),
  // Long titles: the player never cuts them with "…" (two lines in mini, else a slow loop)
  track('[ 92 BPM ] Dios los Bendiga (Extended)', 'SAIKO, Tito El Bambino · DJ Hummer Remix', 'Remixes', '2:24', ['MP3', 320, 44100, null], 'noSigns',
    rep({ verdict: 'noSigns', confidence: 0.95, cutoffHz: 20000, bandwidthHz: 20000 })),
  track('Dios los Bendiga (Extended Club Mix) [92 BPM Intro Edit] – DJ Hummer Remix feat. Tito El Bambino & SAIKO (Live 2025)', 'SAIKO, Tito El Bambino', 'Remixes', '6:02', ['FLAC', 1012, 44100, 16], 'real',
    rep({ verdict: 'real', confidence: 0.98 }))
]

// Covers: one SVG per album, hues chosen so the palette extraction has something to show
const hues: Record<string, [number, number]> = {
  Estrella: [330, 22],
  CHUOS: [210, 190],
  'tu contactO': [280, 320],
  'Da Me': [350, 10],
  Maniatika: [160, 190],
  'Mandanga Style': [45, 20],
  Plakito: [190, 260],
  'Soldado Y Profeta': [0, 30]
}

export function coverUrl(id: number): string {
  const t = tracks.find((x) => x.id === id)
  const [a, b] = (t && hues[t.album]) ?? [330, 22]
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 272 272">
<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="hsl(${a} 50% 12%)"/><stop offset=".45" stop-color="hsl(${a} 52% 27%)"/><stop offset=".75" stop-color="hsl(${b} 66% 47%)"/><stop offset="1" stop-color="hsl(${b + 10} 82% 65%)"/></linearGradient></defs>
<rect width="272" height="272" fill="url(#g)"/><circle cx="136" cy="190" r="62" fill="hsl(${b + 12} 86% 82%)"/>
<rect y="186" width="272" height="86" fill="hsl(${a} 48% 14%)"/>
<g stroke="hsl(${b + 12} 86% 82%)" stroke-linecap="round"><path d="M96 194h80" stroke-width="2.4" opacity=".55"/><path d="M106 203h60" stroke-width="2" opacity=".42"/><path d="M116 213h40" stroke-width="1.8" opacity=".32"/></g>
</svg>`
  return `data:image/svg+xml,${encodeURIComponent(svg)}`
}

// --- player ---
let queue: number[] = tracks.map((t) => t.id)
// The preview (and its screenshots) can start on another track: localStorage poximp.mock.track = its title
let index = Math.max(0, tracks.findIndex((t) => t.title === localStorage.getItem('poximp.mock.track')))
let status: PlayerState['status'] = 'playing'
let volume = 0.72
let shuffle = false
let repeat: RepeatMode = 'off'
let secs = 87
let ab: PlayerState['ab'] = null
let sleep: PlayerState['sleep'] = null
let sleepTimer = 0
// The preview (and its screenshots) can start with the detector off: localStorage poximp.mock.q = 'off'
let fx: FxSettings = { crossfeed: 'off', width: 1, mono: false, balance: 0, bass: 0, speed: 1 }
let qset: QualitySettings = { analyze: localStorage.getItem('poximp.mock.q') !== 'off', showNoSigns: true }
// Output: bit-perfect locks the volume at 100 % (like the real backend).
let outSettings: OutputSettings = { deviceId: null, bitPerfect: false, crossfade: 0, keepAlbumsGapless: true }
const OUT_DEVICES: OutputDevice[] = [
  { id: 'wasapi:{0.0.0.00000000}.{realtek}', name: 'Altavoces (Realtek)', isDefault: true },
  { id: 'wasapi:{0.0.0.00000000}.{dac}', name: 'Topping E30 II', isDefault: false }
]
const outInfo = (): OutputInfo => {
  const device = OUT_DEVICES.find((d) => d.id === outSettings.deviceId) ?? OUT_DEVICES[0]
  return outSettings.bitPerfect
    ? { device: device.name, sampleRate: cur()?.sampleRate ?? 44100, bits: cur()?.bitDepth ?? 24, mode: 'exclusive', exclusive: true, bitPerfect: true, fallback: null }
    : { device: device.name, sampleRate: 48000, bits: 24, mode: 'shared', exclusive: false, bitPerfect: false, fallback: null }
}

const byId = (id: number): Track | undefined => tracks.find((t) => t.id === id)
const cur = (): Track | null => (index >= 0 ? (byId(queue[index]) ?? null) : null)

function nextIndex(): number {
  if (repeat === 'one') return index
  if (index + 1 < queue.length) return index + 1
  return repeat === 'all' ? 0 : -1
}

function state(): PlayerState {
  const n = nextIndex()
  const locked = outSettings.bitPerfect
  return { status, volume: locked ? 1 : volume, volumeLocked: locked, shuffle, repeat, index, queueLength: queue.length, next: n >= 0 ? (byId(queue[n]) ?? null) : null, ab, sleep }
}

const pushState = (): void => emit('player:state', state())
const pushPos = (): void => emit('player:position', { secs, duration: cur()?.duration ?? 0 })

function go(i: number): void {
  if (sleep?.endOfTrack && i !== index) {
    sleep = null
    status = 'paused'
  }
  index = i
  secs = 0
  ab = null
  if (i < 0) status = 'stopped'
  emit('player:track', { track: cur() })
  pushPos()
  pushState()
}

setInterval(() => {
  if (status !== 'playing') return
  secs += 0.25
  const d = cur()?.duration ?? 0
  if (ab?.b != null && secs >= ab.b) {
    secs = ab.a
    pushPos()
  } else if (secs >= d) go(nextIndex())
  else pushPos()
}, 250)

// Background analysis of "Da Me"
setInterval(() => {
  const t = tracks[4]
  if (t.quality !== 'analyzing' || !qset.analyze) return
  t.progress = Math.min(1, (t.progress ?? 0) + 0.01)
  if (t.progress >= 1) {
    t.quality = 'noSigns'
    t.report = rep({ verdict: 'noSigns', confidence: 0.92, cutoffHz: 19700, bandwidthHz: 19700 })
    t.progress = undefined
  }
  emit('quality:update', { trackId: t.id, verdict: t.quality, report: t.report, progress: t.progress })
}, 1400)

// Spectrum like the mockup's: tilted, a beat in the lows, smoothed
function vizFrame(bands: number): ArrayBuffer {
  const out = new Uint8Array(bands + 2)
  const t = performance.now() / 1000
  const beat = Math.pow(Math.max(0, Math.sin(t * Math.PI * 1.55)), 6)
  const on = status === 'playing'
  for (let i = 0; i < bands; i++) {
    const x = i / (bands - 1)
    const tilt = 0.92 - 0.55 * x + 0.15 * Math.sin(x * 5)
    const v = on ? tilt * (0.62 + 0.28 * Math.sin(t * (1.3 + i * 0.17) + i * 2.1) + 0.18 * Math.random()) + (i < 8 ? beat * 0.32 * (1 - i / 8) : 0) : 0
    out[i] = Math.round(Math.max(0, Math.min(1, v)) * 255 * volume)
  }
  const l = on ? (0.62 + 0.22 * beat + 0.06 * Math.random()) * volume : 0
  out[bands] = Math.round(l * 255)
  out[bands + 1] = Math.round(Math.max(0, l - 0.04 + 0.05 * Math.random()) * 255)
  return out.buffer
}

// Seekbar waveform with a song's shape: quiet intro, verses, choruses, a break, a fade-out (per id)
const SONG: [number, number][] = [[0.07, 0.22], [0.14, 0.5], [0.05, 0.64], [0.13, 0.9], [0.12, 0.52], [0.13, 0.92], [0.08, 0.3], [0.17, 0.97], [0.11, -0.6]]
function waveform(id: number): ArrayBuffer {
  const N = 1024
  let s = (id * 7919 + 13) % 233280
  const rnd = (): number => (s = (s * 9301 + 49297) % 233280) / 233280
  const lens = SONG.map(([l]) => l * (0.7 + 0.6 * rnd()))
  const total = lens.reduce((a, b) => a + b, 0)
  const out = new Uint8Array(2 * N)
  const raw: [number, number][] = []
  let sec = 0
  let start = 0
  for (let i = 0; i < N; i++) {
    const x = i / N
    while (sec < SONG.length - 1 && x >= start + lens[sec] / total) start += lens[sec++] / total
    const level = SONG[sec][1]
    // Negative = fade out over the section; a little beat and noise on top
    const f = (x - start) / (lens[sec] / total)
    const base = level < 0 ? -level * (1 - f) ** 1.6 + 0.02 : level
    const rms = base * 0.45 * (0.72 + 0.14 * Math.abs(Math.sin(i * 1.9)) + 0.24 * rnd())
    raw.push([Math.min(1, rms * (1.7 + 0.8 * rnd())), rms])
  }
  const top = Math.max(...raw.map(([p]) => p))
  raw.forEach(([p, r], i) => {
    out[i] = Math.round((p / top) * 255)
    out[N + i] = Math.round((r / top) * 255)
  })
  return out.buffer
}

const folderOf = (t: Track): string => t.path.slice(0, t.path.lastIndexOf('/'))
// Split artists (ids = position + 1) and album keys (= the album name) for search and album / artist pages
const artistsOf = (t: Track): string[] => t.artist.split(', ')
const ARTISTS = [...new Set(tracks.flatMap(artistsOf))].sort((a, b) => a.localeCompare(b))
const artistId = (name: string): number => ARTISTS.indexOf(name) + 1
const fold = (s: string): string => s.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase()
const hit = (q: string, text: string): boolean => fold(q).split(/[^\p{L}\p{N}]+/u).filter(Boolean).every((w) => fold(text).split(/[^\p{L}\p{N}]+/u).some((x) => x.startsWith(w)))

function view(q: Omit<PageQuery, 'offset' | 'limit'>): Track[] {
  const f = q.filter.trim().toLowerCase()
  const dir = q.folder ?? null
  return tracks
    .filter((t) => !f || `${t.title} ${t.artist} ${t.album}`.toLowerCase().includes(f))
    .filter((t) => q.albumKey == null || t.album === q.albumKey)
    .filter((t) => q.artistId == null || artistsOf(t).some((a) => artistId(a) === q.artistId))
    .filter((t) => dir === null || folderOf(t) === dir || (q.recursive === true && folderOf(t).startsWith(`${dir}/`)))
    .sort((a, b) => (q.sort === 'duration' ? a.duration - b.duration : q.sort === 'added' ? b.id - a.id : a[q.sort].localeCompare(b[q.sort])))
}

// The preview can start with an empty library: localStorage poximp.mock.empty = '1'
const emptyLib = localStorage.getItem('poximp.mock.empty') === '1'

function page(q: PageQuery): LibraryPage {
  const rows = emptyLib ? [] : view(q)
  const ok = rows.filter((t) => t.quality === 'real' || t.quality === 'noSigns').length
  const pending = rows.filter((t) => t.quality === 'pending' || t.quality === 'analyzing').length
  return {
    total: rows.length,
    duration: rows.reduce((s, t) => s + t.duration, 0),
    counts: { ok, pending, doubt: rows.filter((t) => t.quality !== 'inconclusive').length - ok - pending },
    tracks: rows.slice(q.offset, q.offset + q.limit)
  }
}

// --- sound (same built-ins as src-tauri/src/sound.rs; the curve maths is rough, it is a preview) ---
const GRAPHIC = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]
const graphic = (gains: number[]): EqBand[] => GRAPHIC.map((freq, i) => ({ kind: 'peak', freq, gain: gains[i], q: 1.41 }))
const BUILTIN: [string, number[]][] = [
  ['flat', [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
  ['bass', [6, 5, 4, 2.5, 1, 0, 0, 0, 0, 0]],
  ['treble', [0, 0, 0, 0, 0, 0.5, 1.5, 3, 4.5, 5.5]],
  ['vocal', [-2, -2, -1, 0, 1.5, 3, 3.5, 2.5, 1, 0]],
  ['rock', [4.5, 3.5, 2, 0.5, -1, -1, 0.5, 2, 3.5, 4]],
  ['electronic', [5, 4.5, 2, 0, -1, 0.5, 1, 2, 3.5, 4]],
  ['acoustic', [2.5, 2, 1.5, 1, 1.5, 1.5, 2, 2.5, 2, 1.5]],
  ['night', [5, 4, 2.5, 1, 0, 0, 0, 1, 2.5, 3.5]]
]
const settings = (bands: EqBand[]): EqSettings => ({ enabled: true, preamp: 0, autoPreamp: true, bands })
let eqCur = settings(graphic(BUILTIN[0][1]))
let eqPreset: string | null = 'b:flat'
let norm: NormMode = 'off'
let userPresets: EqPreset[] = [{ id: 'u:1', name: 'Mis cascos', builtin: false, eq: settings(graphic([3, 2, 1, 0, 0, 0, -1, 1, 2, 1])) }]
const presets = (): EqPreset[] => [...BUILTIN.map(([k, g]) => ({ id: `b:${k}`, name: k, builtin: true, eq: settings(graphic(g)) })), ...userPresets]

function eqState(): EqState {
  // Peaks barely overlap at 1.41 octaves apart: the highest band is a fair stand-in for the curve's peak.
  const maxBoost = Math.max(0, ...eqCur.bands.filter((b) => b.kind !== 'lowPass' && b.kind !== 'highPass').map((b) => b.gain))
  return { eq: eqCur, preset: eqPreset, preamp: eqCur.autoPreamp ? -maxBoost : eqCur.preamp, maxBoost, normalization: norm, presets: presets() }
}

const AUTOEQ: AutoEqEntry[] = [
  ['Sennheiser HD 600', 'oratory1990', 'over-ear'],
  ['Sennheiser HD 650', 'oratory1990', 'over-ear'],
  ['Sony WH-1000XM4', 'oratory1990', 'over-ear'],
  ['Sony WH-1000XM5', 'Rtings', 'over-ear'],
  ['Beyerdynamic DT 770 Pro (80 Ohm)', 'crinacle', 'over-ear'],
  ['Apple AirPods Pro 2', 'oratory1990', 'in-ear'],
  ['Moondrop Aria', 'crinacle', 'in-ear'],
  ['Apple AirPods (3rd generation)', 'Rtings', 'earbud']
].map(([name, source, form]) => ({ name, source, form: form as AutoEqEntry['form'], path: `${source}/${form}/${encodeURIComponent(name)}` }))

// --- folders: C:/Music/<artist>/<file> ---
function folderTree(): FolderNode[] {
  const root: FolderNode = { name: 'C:/Music', path: 'C:/Music', trackCount: 0, totalCount: tracks.length, children: [] }
  for (const t of tracks) {
    const path = folderOf(t)
    let node = root.children.find((c) => c.path === path)
    if (!node) root.children.push((node = { name: path.slice(path.lastIndexOf('/') + 1), path, trackCount: 0, totalCount: 0, children: [] }))
    node.trackCount++
    node.totalCount++
  }
  root.children.sort((x, y) => x.name.localeCompare(y.name))
  return [root]
}

// --- stats: per-track counters and a year of synthetic plays (deterministic) ---
const DAY = 86400
const nowSecs = (): number => Math.floor(Date.now() / 1000)
let seed = 7
const rnd = (): number => ((seed = (seed * 16807) % 2147483647) / 2147483647)
const genres: Record<string, string> = { Estrella: 'Reggaeton', CHUOS: 'Trap', 'tu contactO': 'Reggaeton', 'Da Me': 'Pop', Sesiones: 'Electrónica' }
interface Meta { playCount: number; skipCount: number; rating: number; lastPlayedAt: number | null; addedAt: number; genre: string; year: number }
const meta = new Map<number, Meta>(
  tracks.map((t, i) => [t.id, { playCount: 0, skipCount: i % 4 === 1 ? 2 : 0, rating: [5, 3, 4, 0, 4, 2, 5, 0, 3, 0, 4][i] ?? 0, lastPlayedAt: null, addedAt: nowSecs() - (900 - i * 60) * DAY, genre: genres[t.album] ?? 'Urbano', year: 2019 + (i % 6) }])
)
const plays: { id: number; at: number; ms: number }[] = []
// Plakito, Soldado Y Profeta: loved years ago, silent since (the "forgotten" smart playlist finds them)
const forgotten = [8, 9]
for (let d = 400; d >= 0; d--) {
  if (rnd() < 0.25) continue // days off break the streaks
  const n = 1 + Math.floor(rnd() * 6)
  for (let k = 0; k < n; k++) {
    // Favourites get played more: AURORA, tu contactO, Mandanga Style
    const fav = [1, 4, 7]
    const t = tracks[rnd() < 0.5 ? fav[Math.floor(rnd() * fav.length)] - 1 : Math.floor(rnd() * tracks.length)]
    if (t.quality === 'pending' && d > 10) continue // added recently: a "discovery"
    if (forgotten.includes(t.id)) continue
    const hour = rnd() < 0.12 ? Math.floor(rnd() * 4) : 9 + Math.floor(rnd() * 14)
    const day = new Date((nowSecs() - d * DAY) * 1000)
    day.setHours(hour, Math.floor(rnd() * 60), 0, 0)
    plays.push({ id: t.id, at: Math.floor(day.getTime() / 1000), ms: Math.round(t.duration * 1000 * (0.6 + 0.4 * rnd())) })
  }
}
plays.sort((x, y) => x.at - y.at)
for (const p of plays) {
  const m = meta.get(p.id)!
  m.playCount++
  m.lastPlayedAt = p.at
}
for (const id of forgotten) Object.assign(meta.get(id)!, { playCount: 14, lastPlayedAt: nowSecs() - 3 * 365 * DAY })
for (const t of tracks) {
  const m = meta.get(t.id)!
  Object.assign(t, { year: m.year, genre: m.genre, playCount: m.playCount, lastPlayedAt: m.lastPlayedAt })
}

function tops<K>(rows: { id: number; ms: number }[], key: (id: number) => K[], n = 10): { k: K; plays: number; ms: number }[] {
  const acc = new Map<K, { k: K; plays: number; ms: number }>()
  for (const r of rows) for (const k of key(r.id)) {
    const e = acc.get(k) ?? { k, plays: 0, ms: 0 }
    e.plays++
    e.ms += r.ms
    acc.set(k, e)
  }
  return [...acc.values()].sort((x, y) => y.plays - x.plays || y.ms - x.ms).slice(0, n)
}

const topTracks = (rows: { id: number; ms: number }[], n = 10): TopTrack[] =>
  tops(rows, (id) => [id], n).map((e) => ({ track: byId(e.k)!, plays: e.plays, listenedMs: e.ms }))
const item = (name: string, x: Partial<TopItem>, e: { plays: number; ms: number }): TopItem =>
  ({ name, id: null, key: null, by: null, coverId: null, ...x, plays: e.plays, listenedMs: e.ms })
const ymd = (secs: number): string => {
  const d = new Date(secs * 1000)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function statsBetween(from: number, to: number): Stats {
  const rows = plays.filter((p) => p.at >= from && p.at < to)
  const firstEver = new Map<number, number>()
  for (const p of plays) if (!firstEver.has(p.id)) firstEver.set(p.id, p.at)
  const disc = rows.filter((p) => firstEver.get(p.id)! >= from)
  const byHour = Array<number>(24).fill(0)
  const byWeekday = Array<number>(7).fill(0)
  for (const p of rows) {
    const d = new Date(p.at * 1000)
    byHour[d.getHours()] += p.ms
    byWeekday[(d.getDay() + 6) % 7] += p.ms
  }
  const days = [...new Set(rows.map((p) => ymd(p.at)))].sort()
  let streak = { days: 0, from: null as string | null, to: null as string | null }
  let start = 0
  days.forEach((d, i) => {
    if (i > 0 && (Date.parse(d) - Date.parse(days[i - 1])) / (DAY * 1000) > 1.5) start = i
    if (i - start + 1 > streak.days) streak = { days: i - start + 1, from: days[start], to: d }
  })
  const artistIds = new Map<string, number>()
  return {
    from,
    to,
    listenedMs: rows.reduce((s, p) => s + p.ms, 0),
    plays: rows.length,
    tracks: new Set(rows.map((p) => p.id)).size,
    topTracks: topTracks(rows),
    // Split artists like the backend: "Mora, C. Tangana" counts for both
    topArtists: tops(rows, (id) => byId(id)!.artist.split(/, | & /)).map((e) => {
      if (!artistIds.has(e.k)) artistIds.set(e.k, artistIds.size + 1)
      return item(e.k, { id: artistIds.get(e.k)!, coverId: tracks.find((t) => t.artist.includes(e.k))?.id ?? null }, e)
    }),
    topAlbums: tops(rows, (id) => [byId(id)!.album]).map((e) => {
      const t = tracks.find((x) => x.album === e.k)!
      return item(e.k, { key: e.k, by: t.artist, coverId: t.id }, e)
    }),
    topGenres: tops(rows, (id) => [meta.get(id)!.genre]).map((e) => item(e.k, { coverId: tracks.find((t) => meta.get(t.id)!.genre === e.k)?.id ?? null }, e)),
    byHour,
    byWeekday,
    discoveries: new Set(disc.map((p) => p.id)).size,
    topDiscoveries: topTracks(disc),
    streak
  }
}

function stats(range: StatsRange): Stats {
  const to = nowSecs() + 1
  const span = { week: 7, month: 30, year: 365, all: 0 }[range]
  return statsBetween(span ? to - span * DAY : 0, to)
}

function wrapped(year: number): Wrapped {
  const from = new Date(year, 0, 1).getTime() / 1000
  const to = new Date(year + 1, 0, 1).getTime() / 1000
  const s = statsBetween(from, to)
  const rows = plays.filter((p) => p.at >= from && p.at < to)
  const byMonth = Array<number>(12).fill(0)
  const monthPlays = Array<number>(12).fill(0)
  for (const p of rows) {
    const m = new Date(p.at * 1000).getMonth()
    byMonth[m] += p.ms
    monthPlays[m]++
  }
  const best = byMonth.indexOf(Math.max(...byMonth))
  const late = topTracks(rows.filter((p) => new Date(p.at * 1000).getHours() < 5), 1)[0] ?? null
  return {
    year,
    stats: s,
    byMonth,
    topMonth: rows.length ? { month: best + 1, plays: monthPlays[best], listenedMs: byMonth[best] } : null,
    lateNight: late,
    topVerified: topTracks(rows.filter((p) => byId(p.id)!.quality === 'real'), 5),
    libraryPct: Math.min(100, (s.tracks * 100) / tracks.length)
  }
}

// --- smart playlists: the same rule language as the backend, evaluated in memory ---
function fieldValue(t: Track, f: string): string | number | null {
  const m = meta.get(t.id)!
  const v: Record<string, string | number | null> = {
    title: t.title, artist: t.artist, album: t.album, album_artist: '', genre: m.genre, year: m.year, duration: t.duration,
    codec: t.format, bitrate: t.bitrate, sample_rate: t.sampleRate, bit_depth: t.bitDepth, quality_verdict: t.quality === 'pending' || t.quality === 'analyzing' ? null : t.quality,
    play_count: m.playCount, skip_count: m.skipCount, rating: m.rating, last_played_at: m.lastPlayedAt, added_at: m.addedAt, folder: folderOf(t)
  }
  if (!(f in v)) throw 'invalid_input'
  return v[f]
}

function matches(t: Track, n: SmartNode): boolean {
  if ('all' in n) return n.all.every((c) => matches(t, c))
  if ('any' in n) return n.any.some((c) => matches(t, c))
  const v = fieldValue(t, n.field)
  const x = n.value
  const s = String(v ?? '').toLowerCase()
  const cut = nowSecs() - Number(x) * DAY
  switch (n.op) {
    case 'is_empty': return v === null || v === '' || v === 0
    case 'is': return typeof v === 'number' ? v === Number(x) : s === String(x).toLowerCase() || (n.field === 'artist' && s.split(/, | & /).includes(String(x).toLowerCase()))
    case 'is_not': return !matches(t, { ...n, op: 'is' })
    case 'contains': return s.includes(String(x).toLowerCase())
    case 'not_contains': return !s.includes(String(x).toLowerCase())
    case 'starts_with': return s.startsWith(String(x).toLowerCase())
    case 'gt': return v !== null && Number(v) > Number(x)
    case 'lt': return v !== null && Number(v) < Number(x)
    case 'between': {
      const [lo, hi] = (x as [number, number]).slice().sort((p, q) => p - q)
      return v !== null && Number(v) >= lo && Number(v) <= hi
    }
    case 'in_last_days': return v !== null && Number(v) >= cut
    case 'not_in_last_days': return v === null || Number(v) < cut
  }
  return false
}

function smartIds(r: SmartRules): number[] {
  const rows = tracks.filter((t) => matches(t, r))
  const m = (t: Track): Meta => meta.get(t.id)!
  const by: Record<string, (x: Track, y: Track) => number> = {
    random: (x, y) => ((x.id * 2654435761) % 97) - ((y.id * 2654435761) % 97),
    most_played: (x, y) => m(y).playCount - m(x).playCount,
    least_played: (x, y) => m(x).playCount - m(y).playCount,
    recently_played: (x, y) => (m(y).lastPlayedAt ?? 0) - (m(x).lastPlayedAt ?? 0),
    recently_added: (x, y) => m(y).addedAt - m(x).addedAt,
    most_skipped: (x, y) => m(y).skipCount - m(x).skipCount,
    rating: (x, y) => m(y).rating - m(x).rating,
    title: (x, y) => x.title.localeCompare(y.title),
    artist: (x, y) => x.artist.localeCompare(y.artist),
    album: (x, y) => x.album.localeCompare(y.album),
    year: (x, y) => m(y).year - m(x).year
  }
  return rows.sort(by[r.sort ?? 'artist']).slice(0, r.limit ?? undefined).map((t) => t.id)
}

let nextPlaylistId = 1
const playlistRows: (Omit<Playlist, 'trackCount'> & { items: number[] })[] = []
const addPlaylist = (name: string, rules: SmartRules | null, items: number[] = []): number => {
  const id = nextPlaylistId++
  playlistRows.push({ id, name, kind: rules ? 'smart' : 'manual', rules, createdAt: nowSecs() - (10 - id) * DAY, items })
  return id
}
addPlaylist('Para el coche', null, [2, 4, 7, 1])
addPlaylist('Importada de AIMP', null, [11, 9])
addPlaylist('Olvidadas (2 años sin sonar, nunca saltadas)', {
  all: [
    { field: 'last_played_at', op: 'not_in_last_days', value: 730 },
    { field: 'skip_count', op: 'is', value: 0 }
  ],
  sort: 'random'
})
addPlaylist('Top 25 reales', {
  all: [
    { field: 'quality_verdict', op: 'is', value: 'real' },
    { any: [{ field: 'rating', op: 'gt', value: 3 }, { field: 'play_count', op: 'gt', value: 10 }] }
  ],
  sort: 'most_played',
  limit: 25
})

const plRow = (id: unknown) => {
  const p = playlistRows.find((x) => x.id === Number(id))
  if (!p) throw 'not_found'
  return p
}
const plIds = (p: (typeof playlistRows)[number]): number[] => (p.rules ? smartIds(p.rules) : p.items)
const plPublic = (p: (typeof playlistRows)[number]): Playlist => ({ id: p.id, name: p.name, kind: p.kind, rules: p.rules, createdAt: p.createdAt, trackCount: plIds(p).length })
const plPage = (ids: number[], offset: number, limit: number): PlaylistPage => ({
  total: ids.length,
  duration: ids.reduce((s, id) => s + (byId(id)?.duration ?? 0), 0),
  tracks: ids.slice(offset, offset + limit).map((id) => byId(id)!)
})
const manualRow = (id: unknown) => {
  const p = plRow(id)
  if (p.rules) throw 'invalid_input'
  return p
}
const checkName = (name: unknown): string => {
  const n = String(name).trim()
  if (!n || n.length > 200) throw 'invalid_input'
  return n
}

const resume = new Map<number, number>([[11, 612]])
const pushQueue = (): void => emit('player:queue', { total: queue.length, index })
function searchAll(q: string): SearchGroup[] {
  if (!fold(q).replace(/[^\p{L}\p{N}]+/gu, '')) return []
  const ts = tracks.filter((t) => hit(q, `${t.title} ${t.artist} ${t.album}`)).slice(0, 6)
  const albums = [...new Set(tracks.map((t) => t.album))].filter((a) => hit(q, a)).slice(0, 6)
  const groups: SearchGroup[] = [
    { source: 'local', kind: 'tracks', hits: ts.map((t) => ({ id: String(t.id), title: t.title, subtitle: t.artist, count: null, coverId: t.id, track: t })) },
    {
      source: 'local',
      kind: 'albums',
      hits: albums.map((a) => {
        const of = tracks.filter((t) => t.album === a)
        const who = new Set(of.map((t) => t.artist))
        return { id: a, title: a, subtitle: who.size === 1 ? of[0].artist : null, count: of.length, coverId: of[0].id, track: null }
      })
    },
    {
      source: 'local',
      kind: 'artists',
      hits: ARTISTS.filter((a) => hit(q, a))
        .slice(0, 6)
        .map((a) => {
          const of = tracks.filter((t) => artistsOf(t).includes(a))
          return { id: String(artistId(a)), title: a, subtitle: null, count: of.length, coverId: of[0]?.id ?? null, track: null }
        })
    },
    {
      source: 'local',
      kind: 'playlists',
      hits: playlistRows.filter((p) => hit(q, p.name)).slice(0, 6).map((p) => ({ id: String(p.id), title: p.name, subtitle: null, count: plIds(p).length, coverId: null, track: null }))
    }
  ]
  return groups.filter((g) => g.hits.length)
}

// Lyrics: demo text only (no real song lyrics). Synced for most, plain for one, none for "Pendiente".
let lyricsOnline = true
function lyrics(id: number): Lyrics {
  const t = byId(id)
  if (!t) throw 'not_found'
  if (t.quality === 'pending') return { synced: null, plain: null, source: null }
  const words = ['Primera', 'Segunda', 'Tercera', 'Cuarta', 'Quinta', 'Sexta', 'Séptima', 'Octava']
  const lines = Array.from({ length: Math.floor(t.duration / 6) }, (_, i) => ({
    tMs: 4000 + i * 6000,
    text: i % 9 === 8 ? '' : `${words[i % 8]} línea de «${t.title}», solo para ver cómo queda`
  }))
  const plain = lines.map((l) => l.text).join(String.fromCharCode(10))
  if (t.id === 8) return { synced: null, plain, source: 'embedded' }
  if (t.format === 'MP3' && !lyricsOnline) return { synced: null, plain: null, source: null }
  return { synced: lines, plain, source: t.format === 'MP3' ? 'lrclib' : t.format === 'FLAC' ? 'embedded' : 'lrc' }
}

function mixes(): Mix[] {
  const groups: [string | null, number[]][] = [['Reggaeton', [1, 4, 2, 10, 7]], ['Trap', [3, 9, 6, 8]], [null, [7, 5, 11, 1, 3]]]
  return groups.map(([genre, ids]) => {
    const trackIds = [...ids, ...tracks.map((t) => t.id).filter((i) => !ids.includes(i))].slice(0, 9)
    const names = [...new Set(ids.flatMap((i) => artistsOf(byId(i)!)))].slice(0, 3)
    return { genre, artists: names, trackIds, coverIds: ids.slice(0, 4), durationMs: trackIds.reduce((s, i) => s + byId(i)!.duration * 1000, 0) }
  })
}

const fakeImport = async (name: string, items: number[], unmatchedItems: string[]): Promise<ImportReport> => {
  await new Promise((r) => setTimeout(r, 600))
  return { matched: items.length, unmatched: unmatchedItems.length, playlists: name ? [addPlaylist(name, null, items)] : [], unmatchedItems }
}

// Phone remote: a QR that looks like one (it scans as nothing) and a demo phone while it is on.
let remoteOn = false
let remoteIp = '192.168.1.20'
let remoteCode = ''
let remotePhones: RemotePhone[] = []
const REMOTE_IPS = [
  { ip: '192.168.1.20', name: 'Wi-Fi' },
  { ip: '172.20.0.1', name: 'vEthernet (WSL)' }
]
const newCode = (): string => Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join('')
function fakeQr(code: string): string {
  const n = 29
  let h = [...code].reduce((acc, c) => (acc * 31 + c.charCodeAt(0)) >>> 0, 7)
  let d = ''
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      const f = [[0, 0], [n - 7, 0], [0, n - 7]].find(([fx, fy]) => x >= fx - 1 && x <= fx + 7 && y >= fy - 1 && y <= fy + 7)
      let on: boolean
      if (f) {
        const r = Math.max(Math.abs(x - f[0] - 3), Math.abs(y - f[1] - 3))
        on = r === 3 || r <= 1
      } else {
        h = (h * 1103515245 + 12345) >>> 0
        on = (h & 0x10000) !== 0
      }
      if (on) d += `M${x + 4} ${y + 4}h1v1h-1z`
    }
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n + 8} ${n + 8}" shape-rendering="crispEdges"><rect width="100%" height="100%" fill="#fff"/><path fill="#000" d="${d}"/></svg>`
}
const remoteInfo = (): RemoteInfo => ({
  on: remoteOn,
  url: remoteOn ? `http://${remoteIp}:47800/#${remoteCode}` : null,
  qr: remoteOn ? fakeQr(remoteCode) : null,
  ip: remoteIp,
  ips: REMOTE_IPS,
  port: 47800,
  phones: remoteOn ? remotePhones : []
})

export async function invoke(cmd: string, a: Record<string, unknown> = {}): Promise<unknown> {
  switch (cmd) {
    case 'library_folder_tree':
      return folderTree()
    case 'play_view': {
      const ids = view(a as unknown as PageQuery).map((t) => t.id)
      if (!ids.length) throw 'not_found'
      queue = ids
      status = 'playing'
      pushQueue()
      return go(a.startTrackId == null ? 0 : Math.max(0, ids.indexOf(Number(a.startTrackId))))
    }
    case 'queue_get': {
      const offset = Number(a.offset ?? 0)
      return { total: queue.length, index, offset, tracks: queue.slice(offset, offset + Number(a.limit ?? 500)).map((id) => byId(id)!) }
    }
    case 'queue_play_next':
      queue.splice(index + 1, 0, ...(a.trackIds as number[]))
      if (index < 0) index = 0
      pushQueue()
      return pushState()
    case 'queue_add':
      queue.push(...(a.trackIds as number[]))
      if (index < 0) index = 0
      pushQueue()
      return pushState()
    case 'queue_remove': {
      const gone = new Set(a.positions as number[])
      const removedCurrent = gone.has(index)
      index -= [...gone].filter((p) => p < index).length
      queue = queue.filter((_, i) => !gone.has(i))
      if (!queue.length) index = -1
      pushQueue()
      return removedCurrent ? go(Math.min(index, queue.length - 1)) : pushState()
    }
    case 'queue_move': {
      const [from, to] = [Number(a.from), Number(a.to)]
      const [id] = queue.splice(from, 1)
      queue.splice(to, 0, id)
      index = index === from ? to : from < index && index <= to ? index - 1 : to <= index && index < from ? index + 1 : index
      pushQueue()
      return pushState()
    }
    case 'queue_jump':
      return go(Number(a.position))
    case 'queue_clear':
      queue = index >= 0 ? [queue[index]] : []
      index = queue.length ? 0 : -1
      pushQueue()
      return pushState()
    case 'resume_get':
      return resume.get(Number(a.trackId)) ?? null
    case 'resume_clear':
      resume.delete(Number(a.trackId))
      if (cur()?.id === Number(a.trackId)) {
        secs = 0
        pushPos()
      }
      return
    case 'playlist_list':
      return playlistRows.map(plPublic).sort((x, y) => x.name.localeCompare(y.name))
    case 'playlist_create': {
      const rules = (a.rules as SmartRules | null) ?? null
      if (rules) smartIds(rules)
      return plPublic(plRow(addPlaylist(checkName(a.name), rules)))
    }
    case 'playlist_rename':
      plRow(a.id).name = checkName(a.name)
      return
    case 'playlist_set_rules': {
      const p = plRow(a.id)
      if (!p.rules) throw 'invalid_input'
      smartIds(a.rules as SmartRules)
      p.rules = a.rules as SmartRules
      return
    }
    case 'playlist_delete':
      playlistRows.splice(playlistRows.indexOf(plRow(a.id)), 1)
      return
    case 'playlist_page':
      return plPage(plIds(plRow(a.id)), Number(a.offset), Number(a.limit))
    case 'smart_preview':
      return plPage(smartIds(a.rules as SmartRules), 0, Number(a.limit ?? 50))
    case 'playlist_add':
      manualRow(a.id).items.push(...(a.trackIds as number[]))
      return
    case 'playlist_remove': {
      const p = manualRow(a.id)
      const gone = new Set(a.positions as number[])
      p.items = p.items.filter((_, i) => !gone.has(i))
      return
    }
    case 'playlist_move': {
      const p = manualRow(a.id)
      const [id] = p.items.splice(Number(a.from), 1)
      p.items.splice(Number(a.to), 0, id)
      return
    }
    case 'playlist_play': {
      const ids = plIds(plRow(a.id))
      if (!ids.length) throw 'invalid_input'
      queue = ids.slice()
      status = 'playing'
      pushQueue()
      return go(Number(a.startIndex ?? 0))
    }
    case 'stats':
      return stats(a.range as StatsRange)
    case 'wrapped':
      return wrapped(Number(a.year))
    case 'import_aimp_playlist':
      return fakeImport('Default', [11, 6, 3], ['C:\\Users\\Ana\\Downloads\\[ 95 BPM ] CAPRICHOSO (Extended) (DJ Hummer) - Quevedo.mp3'])
    case 'import_m3u':
      return fakeImport(String(a.path).replace(/^.*[\\/]|\.m3u8?$/gi, '') || 'Lista', [1, 2, 3, 4], [])
    case 'import_itunes_xml':
      return fakeImport('', tracks.slice(0, 8).map((t) => t.id), ['C:/Users/Ana/Music/iTunes/iTunes Media/Music/Unknown/Track 01.m4a', 'D:/Viejo/demo.mp3'])
    case 'import_stats_backup':
      return fakeImport('', tracks.map((t) => t.id), [])
    case 'export_m3u':
      return plIds(plRow(a.playlistId)).length
    case 'export_stats_backup':
      return [...meta.values()].filter((m) => m.playCount || m.skipCount || m.rating).length

    case 'fx_get':
      return fx
    case 'fx_set':
      return (fx = { ...(a.fx as FxSettings) })
    case 'eq_get':
      return eqState()
    case 'eq_presets':
      return presets()
    case 'eq_set':
      eqCur = a.eq as EqSettings
      eqPreset = (a.preset as string | null) ?? null
      return eqState()
    case 'eq_save_preset': {
      const name = String(a.name).trim().slice(0, 40)
      if (!name) throw 'invalid_input'
      const old = userPresets.find((p) => p.name.toLowerCase() === name.toLowerCase())
      const id = old?.id ?? `u:${userPresets.length + 2}`
      userPresets = [...userPresets.filter((p) => p.id !== id), { id, name, builtin: false, eq: eqCur }]
      eqPreset = id
      return eqState()
    }
    case 'eq_delete_preset':
      userPresets = userPresets.filter((p) => p.id !== a.id)
      if (eqPreset === a.id) eqPreset = null
      return eqState()
    case 'set_normalization':
      norm = a.mode as NormMode
      return eqState()
    case 'autoeq_search': {
      const words = String(a.q).toLowerCase().split(/\s+/).filter(Boolean)
      await new Promise((r) => setTimeout(r, 250))
      return words.length ? AUTOEQ.filter((e) => words.every((w) => e.name.toLowerCase().includes(w))) : []
    }
    case 'autoeq_apply': {
      const e = AUTOEQ.find((x) => x.path === a.path)
      if (!e) throw 'not_found'
      await new Promise((r) => setTimeout(r, 400))
      eqCur = {
        ...eqCur,
        enabled: true,
        preamp: -5.1,
        bands: [
          { kind: 'lowShelf', freq: 105, gain: -3, q: 0.7 },
          { kind: 'peak', freq: 139, gain: -3.9, q: 4.92 },
          { kind: 'peak', freq: 213, gain: 3.5, q: 2.46 },
          { kind: 'peak', freq: 3684, gain: 6.8, q: 2.88 },
          { kind: 'peak', freq: 6430, gain: -4.2, q: 0.97 },
          { kind: 'highShelf', freq: 10000, gain: -5.4, q: 0.7 }
        ]
      }
      eqPreset = `autoeq:${e.name}`
      return eqState()
    }
    case 'shortcuts_list': {
      const mac = navigator.platform.toLowerCase().includes('mac')
      const keys: ShortcutInfo['keys'][number][] = ['space', 'right', 'left', 'up', 'down']
      const ids: ShortcutInfo['id'][] = ['playPause', 'next', 'previous', 'volumeUp', 'volumeDown']
      return ids.map((id, i) => ({ id, keys: [mac ? 'cmd' : 'ctrl', 'alt', keys[i]], active: id !== 'volumeDown' }))
    }
    case 'remote_get':
      return remoteInfo()
    case 'remote_set': {
      const ip = (a.ip ?? null) as string | null
      if (ip !== null && !REMOTE_IPS.some((i) => i.ip === ip)) throw 'invalid_input'
      remoteOn = Boolean(a.on)
      if (ip) remoteIp = ip
      if (remoteOn) {
        remoteCode = newCode()
        remotePhones = [{ device: 'android', ip: '192.168.1.34', live: true }]
      }
      return remoteInfo()
    }
    case 'remote_rotate':
      if (!remoteOn) throw 'invalid_input'
      remoteCode = newCode()
      remotePhones = []
      return remoteInfo()
    case 'library_page':
      return page(a as unknown as PageQuery)
    case 'library_search':
      return page({ offset: 0, limit: 50, sort: 'title', filter: String(a.q) }).tracks
    case 'search_all':
      if (String(a.q).length > 800) throw 'invalid_input'
      return searchAll(String(a.q).slice(0, 200))
    case 'lyrics_get':
      await new Promise((r) => setTimeout(r, 300))
      return lyrics(Number(a.trackId))
    case 'lyrics_online_get':
      return lyricsOnline
    case 'lyrics_online_set':
      return (lyricsOnline = Boolean(a.on))
    case 'radio_start': {
      const seed = Number(a.seedTrackId)
      if (!byId(seed)) throw 'not_found'
      queue = [seed, ...tracks.map((t) => t.id).filter((i) => i !== seed).sort(() => rnd() - 0.5)]
      status = 'playing'
      pushQueue()
      return go(0)
    }
    case 'mixes_list':
      return mixes()
    case 'library_folders':
      return ['C:/Music']
    case 'library_artists':
      return ARTISTS.map((name) => ({ id: artistId(name), name, trackCount: tracks.filter((t) => artistsOf(t).includes(name)).length }))
    case 'stop':
      status = 'stopped'
      secs = 0
      ab = null
      pushPos()
      return pushState()
    case 'ab_set': {
      const [a1, b1] = [a.a as number | null, a.b as number | null]
      if (a1 === null && b1 === null) ab = null
      else if (a1 !== null && (b1 === null || b1 - a1 >= 0.5)) ab = { a: a1, b: b1 }
      else throw 'invalid_input'
      return pushState()
    }
    case 'sleep_set': {
      clearTimeout(sleepTimer)
      const m = a.minutes as number | null
      sleep = m ? { endsAt: Date.now() + m * 60000, endOfTrack: false } : a.endOfTrack ? { endsAt: null, endOfTrack: true } : null
      if (m)
        sleepTimer = window.setTimeout(() => {
          sleep = null
          status = 'paused'
          pushState()
        }, m * 60000)
      return pushState()
    }
    case 'quality_settings_get':
      return qset
    case 'quality_settings_set':
      return (qset = { ...(a.settings as QualitySettings) })
    case 'app_version':
      return '1.0.0'
    case 'open_rejected':
      return []
    case 'open_default_apps':
      return
    case 'library_add_folder':
    case 'library_remove_folder':
      return
    case 'library_scan':
      for (let i = 1; i <= 10; i++) setTimeout(() => emit('library:progress', { done: i, total: 10 }), i * 120)
      return
    case 'play_track':
      queue = [Number(a.trackId)]
      status = 'playing'
      pushQueue()
      return go(0)
    case 'play_queue':
      queue = (a.trackIds as number[]).slice()
      status = 'playing'
      pushQueue()
      return go(Number(a.startIndex))
    case 'toggle':
      if (index < 0) {
        status = 'playing'
        return go(0)
      }
      status = status === 'playing' ? 'paused' : 'playing'
      return pushState()
    case 'next':
      return go(index + 1 < queue.length ? index + 1 : repeat === 'all' ? 0 : -1)
    case 'previous':
      if (secs > 3) {
        secs = 0
        return pushPos()
      }
      return go(Math.max(0, index - 1))
    case 'seek':
      secs = Math.max(0, Math.min(Number(a.secs), cur()?.duration ?? 0))
      return pushPos()
    case 'set_volume':
      if (!outSettings.bitPerfect) volume = Number(a.v)
      return pushState()
    case 'set_shuffle':
      shuffle = Boolean(a.b)
      return pushState()
    case 'set_repeat':
      repeat = a.mode as RepeatMode
      return pushState()
    case 'quality_report':
      return byId(Number(a.trackId))?.report ?? null
    case 'viz_frame':
      return vizFrame(Number(a.bands))
    case 'waveform_get':
      return byId(Number(a.trackId)) ? waveform(Number(a.trackId)) : new ArrayBuffer(0)
    case 'player_sync':
      return { state: state(), track: cur(), position: { secs, duration: cur()?.duration ?? 0 } }
    case 'output_info':
      return outInfo()
    case 'output_devices':
      return { devices: OUT_DEVICES, settings: outSettings }
    case 'output_set': {
      const s = a.settings as OutputSettings
      if (s.deviceId !== null && !OUT_DEVICES.some((d) => d.id === s.deviceId)) throw 'invalid_input'
      outSettings = { ...s, crossfade: Math.round(Math.max(0, Math.min(12, s.crossfade))) }
      emit('player:output', outInfo())
      pushState()
      return outSettings
    }
  }
  throw { code: 'unknown', detail: cmd }
}

// First paint: like the mockup, AURORA playing at 1:27
setTimeout(() => {
  emit('player:track', { track: cur() })
  pushPos()
  pushState()
}, 0)
