// Typed access to the backend (contract in types.ts). Outside Tauri (plain vite in a browser) it
// talks to the in-memory mock instead, so the UI can be previewed with demo data.
import { convertFileSrc, invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import * as mock from './mock'
import type {
  AutoEqEntry,
  EqPreset,
  EqSettings,
  EqState,
  FolderNode,
  FxSettings,
  ImportReport,
  Artist,
  LibraryPage,
  Lyrics,
  OpenRejected,
  Mix,
  NormMode,
  OutputDevices,
  OutputInfo,
  OutputSettings,
  PageQuery,
  PlayerState,
  PlayerSync,
  Playlist,
  PlaylistPage,
  Position,
  QualityReport,
  QualitySettings,
  QualityUpdate,
  QueueEvent,
  QueuePage,
  RemoteInfo,
  RepeatMode,
  ScanProgress,
  SearchGroup,
  ShortcutInfo,
  SmartRules,
  Stats,
  StatsRange,
  Track,
  ViewQuery,
  Wrapped
} from './types'

export const native = isTauri()

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return (native ? invoke<T>(cmd, args) : mock.invoke(cmd, args)) as Promise<T>
}

interface Events {
  'player:state': PlayerState
  'player:position': Position
  'player:track': { track: Track | null }
  'player:queue': QueueEvent
  'player:output': OutputInfo
  'quality:update': QualityUpdate
  'waveform:ready': { trackId: number }
  'library:progress': ScanProgress
  /** Files were refused: ask `openRejected` which (no payload). */
  'open:rejected': null
}

/** Subscribe to a backend event; returns the unsubscribe function. */
export function on<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void): () => void {
  if (!native) return mock.listen(event, (p) => cb(p as Events[K]))
  const off = listen<Events[K]>(event, (e) => cb(e.payload))
  return () => void off.then((f) => f())
}

export const api = {
  addFolder: (path: string) => call<void>('library_add_folder', { path }),
  removeFolder: (path: string) => call<void>('library_remove_folder', { path }),
  scan: () => call<void>('library_scan'),
  page: (q: PageQuery) => call<LibraryPage>('library_page', { ...q }),
  search: (q: string) => call<Track[]>('library_search', { q }),
  searchAll: (q: string) => call<SearchGroup[]>('search_all', { q }),
  folders: () => call<string[]>('library_folders'),
  artists: () => call<Artist[]>('library_artists'),
  folderTree: () => call<FolderNode[]>('library_folder_tree'),
  playTrack: (trackId: number) => call<void>('play_track', { trackId }),
  playQueue: (trackIds: number[], startIndex: number) => call<void>('play_queue', { trackIds, startIndex }),
  playView: (view: ViewQuery, startTrackId?: number) => call<void>('play_view', { ...view, startTrackId }),
  // queue (positions = QueuePage positions, play order)
  queueGet: (offset = 0, limit = 500) => call<QueuePage>('queue_get', { offset, limit }),
  queuePlayNext: (trackIds: number[]) => call<void>('queue_play_next', { trackIds }),
  queueAdd: (trackIds: number[]) => call<void>('queue_add', { trackIds }),
  queueRemove: (positions: number[]) => call<void>('queue_remove', { positions }),
  queueMove: (from: number, to: number) => call<void>('queue_move', { from, to }),
  queueClear: () => call<void>('queue_clear'),
  queueJump: (position: number) => call<void>('queue_jump', { position }),
  resumeGet: (trackId: number) => call<number | null>('resume_get', { trackId }),
  resumeClear: (trackId: number) => call<void>('resume_clear', { trackId }),
  // playlists
  playlists: () => call<Playlist[]>('playlist_list'),
  playlistCreate: (name: string, rules: SmartRules | null = null) => call<Playlist>('playlist_create', { name, rules }),
  playlistRename: (id: number, name: string) => call<void>('playlist_rename', { id, name }),
  playlistSetRules: (id: number, rules: SmartRules) => call<void>('playlist_set_rules', { id, rules }),
  playlistDelete: (id: number) => call<void>('playlist_delete', { id }),
  playlistPage: (id: number, offset: number, limit: number) => call<PlaylistPage>('playlist_page', { id, offset, limit }),
  playlistAdd: (id: number, trackIds: number[]) => call<void>('playlist_add', { id, trackIds }),
  playlistRemove: (id: number, positions: number[]) => call<void>('playlist_remove', { id, positions }),
  playlistMove: (id: number, from: number, to: number) => call<void>('playlist_move', { id, from, to }),
  playlistPlay: (id: number, startIndex = 0) => call<void>('playlist_play', { id, startIndex }),
  smartPreview: (rules: SmartRules, limit = 50) => call<PlaylistPage>('smart_preview', { rules, limit }),
  // lyrics, radio, mixes
  lyrics: (trackId: number) => call<Lyrics>('lyrics_get', { trackId }),
  lyricsOnline: () => call<boolean>('lyrics_online_get'),
  setLyricsOnline: (on: boolean) => call<boolean>('lyrics_online_set', { on }),
  radioStart: (seedTrackId: number) => call<void>('radio_start', { seedTrackId }),
  mixes: () => call<Mix[]>('mixes_list'),
  // stats
  stats: (range: StatsRange) => call<Stats>('stats', { range }),
  wrapped: (year: number) => call<Wrapped>('wrapped', { year }),
  // migration (paths from the dialog plugin)
  importAimpPlaylist: (path: string) => call<ImportReport>('import_aimp_playlist', { path }),
  importItunesXml: (path: string) => call<ImportReport>('import_itunes_xml', { path }),
  importM3u: (path: string) => call<ImportReport>('import_m3u', { path }),
  exportM3u: (playlistId: number, path: string) => call<number>('export_m3u', { playlistId, path }),
  exportStatsBackup: (path: string) => call<number>('export_stats_backup', { path }),
  importStatsBackup: (path: string) => call<ImportReport>('import_stats_backup', { path }),

  toggle: () => call<void>('toggle'),
  next: () => call<void>('next'),
  previous: () => call<void>('previous'),
  stop: () => call<void>('stop'),
  abSet: (a: number | null, b: number | null) => call<void>('ab_set', { a, b }),
  sleepSet: (minutes: number | null, endOfTrack = false) => call<void>('sleep_set', { minutes, endOfTrack }),
  seek: (secs: number) => call<void>('seek', { secs }),
  setVolume: (v: number) => call<void>('set_volume', { v }),
  setShuffle: (b: boolean) => call<void>('set_shuffle', { b }),
  setRepeat: (mode: RepeatMode) => call<void>('set_repeat', { mode }),
  qualityReport: (trackId: number) => call<QualityReport | null>('quality_report', { trackId }),
  waveform: (trackId: number) => call<ArrayBuffer>('waveform_get', { trackId }),
  vizFrame: (bands: number) => call<ArrayBuffer>('viz_frame', { bands }),
  outputInfo: () => call<OutputInfo>('output_info'),
  outputDevices: () => call<OutputDevices>('output_devices'),
  outputSet: (settings: OutputSettings) => call<OutputSettings>('output_set', { settings }),
  sync: () => call<PlayerSync>('player_sync'),
  eqGet: () => call<EqState>('eq_get'),
  fxGet: () => call<FxSettings>('fx_get'),
  fxSet: (fx: FxSettings) => call<FxSettings>('fx_set', { fx }),
  eqPresets: () => call<EqPreset[]>('eq_presets'),
  eqSet: (eq: EqSettings, preset: string | null) => call<EqState>('eq_set', { eq, preset }),
  eqSavePreset: (name: string) => call<EqState>('eq_save_preset', { name }),
  eqDeletePreset: (id: string) => call<EqState>('eq_delete_preset', { id }),
  setNormalization: (mode: NormMode) => call<EqState>('set_normalization', { mode }),
  autoEqSearch: (q: string) => call<AutoEqEntry[]>('autoeq_search', { q }),
  autoEqApply: (path: string) => call<EqState>('autoeq_apply', { path }),
  shortcuts: () => call<ShortcutInfo[]>('shortcuts_list'),
  qualitySettings: () => call<QualitySettings>('quality_settings_get'),
  setQualitySettings: (settings: QualitySettings) => call<QualitySettings>('quality_settings_set', { settings }),
  version: () => call<string>('app_version'),
  // "open with PoxiMP" (Explorer / Finder)
  openRejected: () => call<OpenRejected[]>('open_rejected'),
  openDefaultApps: () => call<void>('open_default_apps'),
  /** macOS: traffic lights at [x, vertical centre] (window points), or back home (null) */
  windowLights: (at: [number, number] | null) => call<void>('window_lights', { at }),
  // phone remote
  remoteGet: () => call<RemoteInfo>('remote_get'),
  remoteSet: (on: boolean, ip: string | null = null) => call<RemoteInfo>('remote_set', { on, ip }),
  remoteRotate: () => call<RemoteInfo>('remote_rotate')
}

export const coverUrl = (id: number): string => (native ? convertFileSrc(String(id), 'cover') : mock.coverUrl(id))
