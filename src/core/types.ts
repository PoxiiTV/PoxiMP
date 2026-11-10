/**
 * Backend contract (src-tauri implements exactly this; field names are camelCase on the wire).
 *
 * Commands (invoke):
 *   library_add_folder({ path })                     -> void          add a music folder (does not scan)
 *   library_remove_folder({ path })                  -> void          take a music folder, or one inside it, out
 *   library_scan()                                   -> void          rescan every folder; progress via "library:progress"
 *   library_page({ offset, limit, sort, filter, folder?, recursive?, albumKey?, artistId? }) -> LibraryPage
 *                                                                   `filter` = free text ('' = all); `folder` = a FolderNode.path,
 *                                                                   `recursive` = include its subfolders (default false);
 *                                                                   `albumKey` / `artistId` = just that album / artist (search hits)
 *   library_folder_tree()                            -> FolderNode[]  one top node per music folder, subfolders below
 *   play_view({ sort, filter, folder?, recursive?, albumKey?, artistId?, startTrackId? }) -> void
 *                                                                   play a whole view server-side (same query as library_page,
 *                                                                   no paging); starts at startTrackId (first track if absent)
 *   library_search({ q })                            -> Track[]       top track matches
 *   search_all({ q })                                -> SearchGroup[] global search bar: tracks, albums, artists, playlists
 *                                                                   (only groups with hits; q max 800 bytes, 200 chars used)
 *   library_folders()                                -> string[]
 *   library_artists()                                -> Artist[]      every split artist with its track count, by name
 *   play_track({ trackId })                          -> void          play one track (queue = just it)
 *   play_queue({ trackIds, startIndex })             -> void          replace the queue and start at startIndex
 *   queue_get({ offset?, limit? })                   -> QueuePage     play order (shuffle applied); default 0 / 500, max 1000
 *   queue_play_next({ trackIds })                    -> void          insert right after the current track
 *   queue_add({ trackIds })                          -> void          append (max 10 000 per call, queue max 100 000)
 *   queue_remove({ positions })                      -> void          QueuePage positions; removing the current one plays the next
 *   queue_move({ from, to })                         -> void          QueuePage positions
 *   queue_clear()                                    -> void          everything but the current track
 *   resume_get({ trackId })                          -> number | null saved position (secs) of a track > 20 min; play resumes there
 *   resume_clear({ trackId })                        -> void          "start from the beginning" (rewinds it if it is playing)
 *   playlist_list()                                  -> Playlist[]    by name; smart ones count their current matches
 *   playlist_create({ name, rules })                 -> Playlist      rules = null (manual) or SmartRules (smart)
 *   playlist_rename({ id, name })                    -> void          1-200 characters
 *   playlist_set_rules({ id, rules })                -> void          smart playlists only
 *   playlist_delete({ id })                          -> void
 *   playlist_page({ id, offset, limit })             -> PlaylistPage  manual: items in order; smart: evaluated now
 *   playlist_add({ id, trackIds })                   -> void          manual only, appended
 *   playlist_remove({ id, positions })               -> void          manual only
 *   playlist_move({ id, from, to })                  -> void          manual only
 *   playlist_play({ id, startIndex? })               -> void          replaces the queue with the playlist
 *   smart_preview({ rules, limit? })                 -> PlaylistPage  live preview while editing rules (first `limit`, default 50, max 200)
 *   lyrics_get({ trackId })                          -> Lyrics        tags, then a sidecar .lrc, then LRCLIB (if lyrics_online);
 *                                                                   rejects network_error only when offline with nothing on disk
 *   lyrics_online_get() / lyrics_online_set({ on })  -> boolean       "online lyrics" setting (LRCLIB), on by default
 *   radio_start({ seedTrackId })                     -> void          queue = the seed + ~50 similar tracks, plays
 *   mixes_list()                                     -> Mix[]         3-6 automatic mixes from the most played (empty until
 *                                                                   there is history and the sound analysis has reached it)
 *   stats({ range })                                 -> Stats         StatsRange: rolling last 7 / 30 / 365 days, or everything
 *   wrapped({ year })                                -> Wrapped       calendar year, local time
 *   import_aimp_playlist({ path })                   -> ImportReport  .aimppl4 (AIMP 4/5) -> new manual playlist
 *   import_itunes_xml({ path })                      -> ImportReport  iTunes/Music Library.xml: plays, skips, ratings, dates, playlists
 *   import_m3u({ path })                             -> ImportReport  .m3u / .m3u8 -> new manual playlist
 *   export_m3u({ playlistId, path })                 -> number        tracks written (UTF-8 .m3u8, relative paths on the same drive)
 *   export_stats_backup({ path })                    -> number        tracks written (.json: counts, ratings, dated plays)
 *   import_stats_backup({ path })                    -> ImportReport  restore a backup (safe to run twice)
 *   Import/export paths come from the dialog plugin: absolute, with the right extension.
 *   toggle() / next() / previous()                   -> void
 *   stop()                                           -> void          stop and back to 0 (play starts the current track again)
 *   ab_set({ a?, b? })                               -> void          A-B repeat of the current track, seconds: a alone = A marked,
 *                                                                   a + b = loop (b - a >= 0.5), neither = off. Reset on track change
 *   sleep_set({ minutes?, endOfTrack })              -> void          sleep timer: pause in minutes (1..1440) or stop when the
 *                                                                   current track ends; neither = off. State in PlayerState.sleep
 *   seek({ secs })                                   -> void
 *   set_volume({ v })                                -> void          linear 0..1
 *   set_shuffle({ b })                               -> void
 *   set_repeat({ mode })                             -> void          RepeatMode
 *   quality_report({ trackId })                      -> QualityReport | null
 *   waveform_get({ trackId })                        -> ArrayBuffer   seekbar waveform, raw bytes: N peaks then N RMS values
 *                                                                   (0..255, relative to the track's loudest peak, N = 1024);
 *                                                                   empty = not computed yet ("waveform:ready" follows) or unreadable
 *   viz_frame({ bands })                             -> ArrayBuffer   tauri::ipc::Response(Vec<u8>), see VIZ_* below
 *   output_info()                                    -> OutputInfo
 *   output_devices()                                 -> OutputDevices devices the system offers now + chosen settings
 *   output_set({ settings })                         -> OutputSettings deviceId must come from output_devices (or null);
 *                                                                   crossfade 0..12 s; applied at once (bit-perfect reopens the device)
 *   player_sync()                                    -> PlayerSync    current snapshot (UI boot / reload; events may have been missed)
 *   eq_get()                                         -> EqState
 *   eq_presets()                                     -> EqPreset[]    built-ins first, then the user's
 *   eq_set({ eq, preset })                           -> EqState       `preset` = id the curve still matches, null once edited
 *   eq_save_preset({ name })                         -> EqState       current curve as a user preset (same name overwrites)
 *   eq_delete_preset({ id })                         -> EqState       user presets only
 *   set_normalization({ mode })                      -> EqState
 *   autoeq_search({ q })                             -> AutoEqEntry[] headphone models (index cached a week, works offline)
 *   autoeq_apply({ path })                           -> EqState       downloads that model's correction and applies it
 *   quality_settings_get()                           -> QualitySettings
 *   quality_settings_set({ settings })               -> QualitySettings analyze off = the background detector pauses
 *   shortcuts_list()                                 -> ShortcutInfo[]
 *   app_version()                                    -> string
 *   remote_get()                                     -> RemoteInfo    phone remote: off by default, starts off every launch
 *   remote_set({ on, ip? })                          -> RemoteInfo    ip = one of RemoteInfo.ips (absent = saved / default route);
 *                                                                   turning it on (again) means a new code
 *   remote_rotate()                                  -> RemoteInfo    new code: the old QR stops working, paired phones are signed out
 *
 * Events (listen):
 *   "player:state"     PlayerState                  on any transport/queue/volume change
 *   "player:output"    OutputInfo                   output (re)opened: device change, bit-perfect format switch, fallback
 *   "player:position"  { secs, duration }           4 Hz while playing, and once after every seek
 *   "player:track"     { track: Track | null }      current track changed
 *   "player:queue"     QueueEvent                   queue contents or order changed (refetch with queue_get)
 *   "quality:update"   QualityUpdate                analysis started / progressed / finished
 *   "waveform:ready"   { trackId }                  that track's waveform was computed (playing / next track first)
 *   "library:progress" { done, total }              scan progress; done === total means finished
 *
 * Errors reject with a short code string: invalid_input, not_found, library_error, network_error, io_error.
 *
 * Covers: custom URI scheme "cover", path = track id (convertFileSrc(String(id), 'cover'), i.e.
 * http://cover.localhost/<id> on Windows, cover://localhost/<id> on macOS). 404 when there is none.
 * It must send `Access-Control-Allow-Origin: *` so the UI can read pixels for the cover palette,
 * and the CSP needs `img-src ... http://cover.localhost cover:`.
 */

/** Detector outcome. MP3/lossy never gets 'real': at best 'noSigns'. */
export type Verdict =
  | 'real' // lossless, full bandwidth (and real bit depth / sample rate)
  | 'noSigns' // lossy and nothing wrong found
  | 'suspicious' // some evidence, not enough confidence
  | 'fake' // lossy declared at B kbps, sounds like less
  | 'lossyOrigin' // lossless container made from a lossy file
  | 'fakeHiRes' // "24/96" that is really 16/44.1 (or 48)
  | 'inconclusive' // not enough signal to decide

/** What the UI shows for a track: a verdict, or the analysis not having finished. */
export type QualityState = Verdict | 'analyzing' | 'pending'

export interface QualityReport {
  verdict: Verdict
  /** 0..1 */
  confidence: number
  /** Hard cutoff found (Hz); null = no cliff, full band */
  cutoffHz: number | null
  /** Highest frequency with real content (Hz) */
  bandwidthHz: number
  /** sampleRate / 2 (Hz) */
  nyquistHz: number
  /** Equivalent bitrate for fake / suspicious / lossyOrigin (kbps) */
  estKbps: number | null
  /** Lossy source codec for lossyOrigin */
  origin: 'mp3' | 'aac' | 'opus' | 'vorbis' | null
  /** Real resolution for fakeHiRes */
  realBits: number | null
  realRate: number | null
}

export interface Track {
  id: number
  path: string
  title: string
  artist: string
  album: string
  /** Seconds */
  duration: number
  /** Upper-case codec/container label: FLAC, MP3, WAV, M4A, OGG, OPUS, AIFF… */
  format: string
  /** kbps (average) */
  bitrate: number
  /** Hz */
  sampleRate: number
  /** null for lossy */
  bitDepth: number | null
  /** Lossless codec (decides "sin pérdida" vs "con pérdida") */
  lossless: boolean
  quality: QualityState
  report: QualityReport | null
  /** 0..1 while quality === 'analyzing' */
  progress?: number
  year: number | null
  genre: string
  playCount: number
  /** Unix seconds, null = never */
  lastPlayedAt: number | null
  /** ReplayGain of the track (dB), when tagged */
  replayGain: number | null
  /** Opened from outside the music folders (double-click, drop): plays, but is not in the library */
  external?: boolean
}

export interface Artist {
  id: number
  name: string
  trackCount: number
}

/** 'path' = file order (folder view) */
export type SortKey = 'artist' | 'title' | 'album' | 'duration' | 'added' | 'path'

/** A library view: what library_page pages through and play_view plays. */
export interface ViewQuery {
  sort: SortKey
  filter: string
  /** FolderNode.path; absent/null = whole library */
  folder?: string | null
  /** With `folder`: include subfolders */
  recursive?: boolean
  /** SearchHit.id of an album */
  albumKey?: string | null
  artistId?: number | null
}

export interface PageQuery extends ViewQuery {
  offset: number
  limit: number
}

export interface FolderNode {
  /** Display name: the music folder's full path for top nodes, "Rock/Live" for merged single-child chains */
  name: string
  /** Value for ViewQuery.folder */
  path: string
  /** Tracks directly in this folder */
  trackCount: number
  /** Tracks here and in every subfolder */
  totalCount: number
  children: FolderNode[]
}

export interface LibraryPage {
  /** Rows matching the filter */
  total: number
  /** Seconds, all matching rows */
  duration: number
  /** Over all matching rows: ok = real/noSigns, doubt = suspicious/fake/lossyOrigin/fakeHiRes, pending = pending/analyzing */
  counts: { ok: number; doubt: number; pending: number }
  tracks: Track[]
}

export type RepeatMode = 'off' | 'all' | 'one'
export type PlayStatus = 'playing' | 'paused' | 'stopped'

export interface PlayerState {
  status: PlayStatus
  /** 0..1 (1 while volumeLocked) */
  volume: number
  /** Bit-perfect (exclusive) output: volume fixed at 100 %, the slider shows it locked */
  volumeLocked: boolean
  shuffle: boolean
  repeat: RepeatMode
  /** Index in the queue, -1 if empty */
  index: number
  queueLength: number
  /** What plays after the current track (honours shuffle/repeat), null if nothing */
  next: Track | null
  /** A-B repeat of the current track (seconds): b null = only A marked so far */
  ab: { a: number; b: number | null } | null
  /** Sleep timer: pauses at endsAt (unix ms), or stops when the current track ends */
  sleep: { endsAt: number | null; endOfTrack: boolean } | null
}

export interface Position {
  secs: number
  duration: number
  /** Effects speed: source seconds per second (the clock between events runs at it) */
  speed?: number
}

export type Crossfeed = 'off' | 'soft' | 'medium' | 'strong'
/** Ajustes → Efectos (fx_get / fx_set) */
export interface FxSettings {
  crossfeed: Crossfeed
  /** 0 = mono .. 1 = as recorded .. 2 */
  width: number
  mono: boolean
  /** -1 left .. 1 right */
  balance: number
  /** low shelf dB, 0..12 */
  bass: number
  /** 0.5..1.5, pitch follows */
  speed: number
}

/** A slice of the queue in play order (shuffle applied). Positions = offset + i. */
export interface QueuePage {
  total: number
  /** Play position of the current track, -1 if none (PlayerState.index is the unshuffled index instead) */
  index: number
  offset: number
  tracks: Track[]
}

export interface QueueEvent {
  total: number
  /** Same as QueuePage.index */
  index: number
}

// --- playlists ---

export type PlaylistKind = 'manual' | 'smart'

export interface Playlist {
  id: number
  name: string
  kind: PlaylistKind
  /** Smart playlists only */
  rules: SmartRules | null
  /** Unix seconds */
  createdAt: number
  trackCount: number
}

export interface PlaylistPage {
  total: number
  /** Seconds, the whole list */
  duration: number
  tracks: Track[]
}

/** Text fields: title artist album album_artist genre codec quality_verdict folder.
 *  Numbers: year, duration (secs), bitrate (kbps), sample_rate, bit_depth, play_count, skip_count, rating (0-5).
 *  Dates (unix secs): last_played_at, added_at. */
export type SmartField =
  | 'title'
  | 'artist'
  | 'album'
  | 'album_artist'
  | 'genre'
  | 'year'
  | 'duration'
  | 'codec'
  | 'bitrate'
  | 'sample_rate'
  | 'bit_depth'
  | 'quality_verdict'
  | 'play_count'
  | 'skip_count'
  | 'rating'
  | 'last_played_at'
  | 'added_at'
  | 'folder'

/** Text: is is_not contains not_contains starts_with is_empty (artist is/is_not also match each split artist).
 *  Numbers: is is_not gt lt between is_empty (null or 0). Dates: gt lt between in_last_days not_in_last_days is_empty (never).
 *  not_in_last_days includes never played. Text matching folds ASCII case only. */
export type SmartOp =
  | 'is'
  | 'is_not'
  | 'contains'
  | 'not_contains'
  | 'starts_with'
  | 'gt'
  | 'lt'
  | 'between'
  | 'in_last_days'
  | 'not_in_last_days'
  | 'is_empty'

export interface SmartRule {
  field: SmartField
  op: SmartOp
  /** string (text), number, [low, high] (between), days (in_last_days / not_in_last_days), null (is_empty) */
  value: string | number | [number, number] | null
}

/** Exactly one of all (AND) / any (OR). Empty all = everything, empty any = nothing. Max depth 8, 200 rules. */
export type SmartGroup = { all: SmartNode[] } | { any: SmartNode[] }
export type SmartNode = SmartRule | SmartGroup

/** 'random' is stable for a day (paging does not reshuffle) */
export type SmartSort =
  | 'random'
  | 'most_played'
  | 'least_played'
  | 'recently_played'
  | 'recently_added'
  | 'most_skipped'
  | 'rating'
  | 'title'
  | 'artist'
  | 'album'
  | 'year'

/** Top-level group plus optional limit (1-10000) and sort (default: by artist). */
export type SmartRules = SmartGroup & { limit?: number; sort?: SmartSort }

// --- stats ---

export type StatsRange = 'week' | 'month' | 'year' | 'all'

export interface TopTrack {
  track: Track
  plays: number
  listenedMs: number
}

export interface TopItem {
  name: string
  /** Artist id (topArtists) */
  id: number | null
  /** Album key (topAlbums) */
  key: string | null
  /** Album artist (topAlbums); null = various */
  by: string | null
  /** A track of it, for coverUrl */
  coverId: number | null
  plays: number
  listenedMs: number
}

export interface Stats {
  /** Window [from, to), unix secs */
  from: number
  to: number
  listenedMs: number
  plays: number
  /** Distinct tracks played */
  tracks: number
  topTracks: TopTrack[]
  /** Split artists: a "feat." play counts for each artist */
  topArtists: TopItem[]
  topAlbums: TopItem[]
  topGenres: TopItem[]
  /** 24 values, ms listened per local hour */
  byHour: number[]
  /** 7 values, ms listened per weekday, Monday first */
  byWeekday: number[]
  /** Tracks first ever played in the window */
  discoveries: number
  topDiscoveries: TopTrack[]
  /** Longest run of days with plays; from/to = local 'YYYY-MM-DD' */
  streak: { days: number; from: string | null; to: string | null }
}

export interface Wrapped {
  year: number
  stats: Stats
  /** 12 values, ms listened per month, January first */
  byMonth: number[]
  topMonth: { month: number; plays: number; listenedMs: number } | null
  /** Most played between 00:00 and 05:00 */
  lateNight: TopTrack | null
  /** Most played tracks verified 'real' */
  topVerified: TopTrack[]
  /** Distinct tracks played / library size, 0..100 */
  libraryPct: number
}

// --- migration ---

export interface ImportReport {
  matched: number
  unmatched: number
  /** Ids of the playlists created */
  playlists: number[]
  /** First 50 unmatched entries (path, as written in the source) */
  unmatchedItems: string[]
}

// --- lyrics, search, radio ---

export interface LyricLine {
  /** Milliseconds from the start */
  tMs: number
  /** '' = an instrumental gap */
  text: string
}

export interface Lyrics {
  /** Timed lines, sorted */
  synced: LyricLine[] | null
  plain: string | null
  /** Where they came from; null = none found */
  source: 'embedded' | 'lrc' | 'lrclib' | null
}

export type SearchKind = 'tracks' | 'albums' | 'artists' | 'playlists'

export interface SearchHit {
  /** Track / artist / playlist id, or album key (as text: other sources bring their own ids) */
  id: string
  title: string
  /** Artist (tracks), album artist (albums; null = various artists) */
  subtitle: string | null
  /** Tracks in the album / by the artist / in the playlist */
  count: number | null
  /** coverUrl() of this track stands for the hit */
  coverId: number | null
  /** kind 'tracks' */
  track: Track | null
}

export interface SearchGroup {
  /** 'local' = the library (a future online source adds its own groups) */
  source: 'local'
  kind: SearchKind
  hits: SearchHit[]
}

export interface Mix {
  /** Most common genre of its tracks, if tagged */
  genre: string | null
  /** Its most played artists (up to 3) */
  artists: string[]
  trackIds: number[]
  /** Up to 4 track ids for the tile's covers */
  coverIds: number[]
  durationMs: number
}

export interface PlayerSync {
  state: PlayerState
  track: Track | null
  position: Position
}

export interface QualityUpdate {
  trackId: number
  verdict: QualityState
  report: QualityReport | null
  progress?: number
}

export interface ScanProgress {
  done: number
  total: number
}

/** A file "open with PoxiMP" refused (Explorer, Finder or a drop): just its name, never the path. */
export interface OpenRejected {
  name: string
  reason: 'missing' | 'unsupported' | 'empty'
}

export interface OutputInfo {
  device: string
  sampleRate: number
  bits: number | null
  mode: 'shared' | 'exclusive'
  exclusive: boolean
  /** Exclusive at the file's own rate and depth, nothing in between */
  bitPerfect: boolean
  /** What could not be honoured: chosen device gone (default used), exclusive refused or busy
   *  (shared instead), or exclusive at the nearest format the device takes */
  fallback: 'deviceMissing' | 'exclusiveFailed' | 'exclusiveNotAllowed' | 'formatChanged' | null
}

export interface OutputDevice {
  /** Opaque id for output_set */
  id: string
  name: string
  isDefault: boolean
}

export interface OutputSettings {
  /** null = follow the system default */
  deviceId: string | null
  /** WASAPI exclusive / CoreAudio hog mode at each file's format; no EQ, normalization, crossfade or volume */
  bitPerfect: boolean
  /** Seconds, 0 = off (gapless), 1..12 */
  crossfade: number
  /** No crossfade between tracks of the same album */
  keepAlbumsGapless: boolean
}

export interface OutputDevices {
  devices: OutputDevice[]
  settings: OutputSettings
}

/** viz_frame layout: `bands` bytes of spectrum (0..255, log-spaced 20 Hz..20 kHz, low to high),
 *  then 2 bytes of L/R peak level, 0..255 = -60..0 dBFS. */
export const VIZ_PEAK_DB = 60

// --- sound ---

export type EqKind = 'peak' | 'lowShelf' | 'highShelf' | 'lowPass' | 'highPass'

export interface EqBand {
  kind: EqKind
  /** Hz */
  freq: number
  /** dB, ignored by lowPass / highPass */
  gain: number
  q: number
}

export interface EqSettings {
  /** false = bypass */
  enabled: boolean
  /** dB, used when autoPreamp is off */
  preamp: number
  autoPreamp: boolean
  /** Up to 10 */
  bands: EqBand[]
}

export interface EqPreset {
  /** 'b:<key>' built-in, 'u:<n>' user */
  id: string
  /** i18n key eq.preset.<name> for built-ins, the user's text otherwise */
  name: string
  builtin: boolean
  eq: EqSettings
}

/** off = untouched (default); boostQuiet = only raise quiet tracks, never lower loud ones */
export type NormMode = 'off' | 'track' | 'album' | 'boostQuiet'

export interface EqState {
  eq: EqSettings
  /** Preset id, 'autoeq:<model>', or null (edited by hand) */
  preset: string | null
  /** Preamp actually applied (dB) */
  preamp: number
  /** Highest boost of the curve (dB) */
  maxBoost: number
  normalization: NormMode
  presets: EqPreset[]
}

export interface AutoEqEntry {
  name: string
  /** Who measured it (oratory1990, crinacle…) */
  source: string
  form: 'over-ear' | 'in-ear' | 'earbud'
  /** Id for autoeq_apply */
  path: string
}

/** The fake-quality detector. analyze off = no badges, columns or reports, and no analysis. */
export interface QualitySettings {
  analyze: boolean
  /** Neutral "no signs" badge on lossy files (off = only the warnings) */
  showNoSigns: boolean
}

export type ShortcutKey = 'ctrl' | 'cmd' | 'alt' | 'space' | 'right' | 'left' | 'up' | 'down'

export interface ShortcutInfo {
  id: 'playPause' | 'next' | 'previous' | 'volumeUp' | 'volumeDown'
  keys: ShortcutKey[]
  /** false = another app already holds the combination */
  active: boolean
}

export interface RemotePhone {
  device: 'iphone' | 'ipad' | 'android' | 'other'
  ip: string
  /** Its page is open right now */
  live: boolean
}

export interface RemoteInfo {
  on: boolean
  /** http://<ip>:<port>/#<code>, what the QR says (null while off) */
  url: string | null
  /** The QR as an SVG document (null while off) */
  qr: string | null
  /** Address in use (or the saved choice while off) */
  ip: string | null
  /** Home-network addresses to choose from, the default route's first; empty = no home network */
  ips: { ip: string; name: string }[]
  port: number
  phones: RemotePhone[]
}
