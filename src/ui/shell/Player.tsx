// Player panel: the whole of mini mode (docs/bocetos/08-mini.html, design A). Three columns of
// which the outer two are equal, so the play button sits at the exact centre of the window at any
// width; the waveform seekbar spans the full width underneath. In mini there is no title bar: the
// search and the window buttons sit top right (docs/bocetos/11-buscador-mini.html, design 2), its
// results open under the panel, and any empty spot drags the window.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react'
import { api, coverUrl, native } from '../../core/api'
import { formatLabel, num, time } from '../../core/format'
import { player, usePosition, useSpectrum, useVolumeLock, useWaveform } from '../../core/hooks'
import { t as tr, useT } from '../../core/i18n'
import { setPrefs, setState, useStore } from '../../core/store'
import type { Track } from '../../core/types'
import { leaveMini, toggleMini } from '../../core/window'
import { findSkin } from '../../skins/registry'
import { Seekbar, Slider } from '../controls'
import { popupMenu } from '../nativeMenu'
import { useBadge } from '../quality'
import { useGlobalSearch } from '../Search'
import { openTrackMenu } from '../TrackMenu'
import { WindowControls, isMac } from '../WindowControls'
import * as Ico from './icons'

export function PlayerPanel() {
  const t = useT()
  const track = useStore((s) => s.track)
  const mini = useStore((s) => s.mini)
  // Always mounted (switching modes must not remount the panel); idle in the full window
  const { input, popup } = useGlobalSearch(mini ? 'mini' : 'off')
  const search = mini && (
    <label className="msearch">
      <Ico.Search />
      <input {...input} placeholder={t('search.mini')} aria-label={t('library.search')} spellCheck={false} />
      {input.value ? (
        <button className="x" aria-label={t('library.clearSearch')} onClick={() => setState({ search: '' })}>
          <Ico.Close />
        </button>
      ) : (
        <kbd>{isMac ? '⌘ K' : 'Ctrl K'}</kbd>
      )}
    </label>
  )
  return (
    <>
      <section className="pl" {...(mini ? { 'data-tauri-drag-region': 'deep' } : {})}>
        <NowPlaying track={track} lines={mini ? 2 : 1} />
        <Transport />
        <Side search={search} />
        <Wave track={track} />
      </section>
      {popup}
    </>
  )
}

// ---------- left: cover + what is playing ----------

export function Cover({ track }: { track: Track | null }) {
  const t = useT()
  const [broken, setBroken] = useState<number | null>(null)
  return (
    <div className="cov">
      {track && broken !== track.id ? (
        <img src={coverUrl(track.id)} alt={t('player.cover', { album: track.album || track.title })} draggable={false} onError={() => setBroken(track.id)} />
      ) : (
        <Ico.Music />
      )}
    </div>
  )
}

/** "FLAC · 44,1 kHz · 16 bit · 912 kbps". Non-breaking spaces inside each part and before each
 *  dot: when it has to wrap, it breaks only after a "·". */
export function techLine(tr: Track): string {
  const parts = [tr.format, tr.sampleRate ? `${num(tr.sampleRate / 1000)}\u00a0kHz` : '', tr.bitDepth ? `${tr.bitDepth}\u00a0bit` : '', tr.bitrate ? `${Math.round(tr.bitrate)}\u00a0kbps` : '']
  return parts.filter(Boolean).join('\u00a0· ')
}

function NowPlaying({ track, lines }: { track: Track | null; lines: 1 | 2 }) {
  const t = useT()
  const show = (q: string) => () => setState({ view: 'library', search: q })
  return (
    <div className="left" onContextMenu={(e) => track && openTrackMenu(e, track)}>
      <Cover track={track} />
      <div className="inf">
        <Title text={track?.title ?? t('player.idle')} lines={lines} />
        <div className="ar">
          {track ? (
            <>
              <button className="lnk" onClick={show(track.artist)} title={t('player.filterBy', { name: track.artist })}>
                {track.artist}
              </button>
              {track.album && (
                <span className="ar-al">
                  {' · '}
                  <button className="lnk" onClick={show(track.album)} title={t('player.filterBy', { name: track.album })}>
                    {track.album}
                  </button>
                  {track.year ? ` (${track.year})` : ''}
                </span>
              )}
            </>
          ) : (
            t('player.idleHint')
          )}
        </div>
        {track && (
          <div className="tech">
            <span className="tech-t">{techLine(track)}</span>
            <span className="tech-s">{formatLabel(track)}</span>
            <Badge track={track} />
          </div>
        )}
      </div>
    </div>
  )
}

/** Gap between the two copies of a scrolling title, and its speed */
const MARQ_GAP = 48
const MARQ_PX_S = 28

/** The title never ends in "…": one line if it fits; else, where allowed, two lines (if the block
 *  still fits its column); else one line that scrolls slowly, pausing at the start (a CSS loop;
 *  with reduced motion it just stops, clipped with a fade). Measured on the DOM, no re-render. */
function Title({ text, lines }: { text: string; lines: 1 | 2 }) {
  const ref = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const el = ref.current
    const inner = el?.firstElementChild as HTMLElement | null
    if (!el || !inner) return
    const fit = (): void => {
      // Measuring restarts the loop: keep where it was, so a resize or a font load does not jump
      const was = el.dataset.fit === 'marq' ? inner.getAnimations()[0]?.currentTime : null
      el.dataset.fit = 'one'
      if (el.scrollWidth <= el.clientWidth + 1) return
      const textW = inner.getBoundingClientRect().width
      if (lines === 2) {
        el.dataset.fit = 'two'
        const inf = el.parentElement
        const box = el.closest('.left')
        // At most two lines (italic/LCD glyphs may poke a pixel or two out of the line boxes)
        const fits = el.scrollHeight < parseFloat(getComputedStyle(el).lineHeight) * 2.5 && el.scrollWidth <= el.clientWidth + 1
        if (fits && (!inf || !box || inf.scrollHeight <= box.clientHeight + 1)) return
      }
      const d = textW + MARQ_GAP
      el.style.setProperty('--marq', `${d.toFixed(1)}px`)
      // 70 % of the loop moves, the rest is the pause at the start
      el.style.setProperty('--marq-t', `${(d / MARQ_PX_S / 0.7).toFixed(1)}s`)
      el.dataset.fit = 'marq'
      const loop = inner.getAnimations()[0]
      if (loop && was != null) loop.currentTime = was
    }
    fit()
    // Again when the column changes (window size, the badge arriving) or the skin's fonts load
    const ro = new ResizeObserver(fit)
    ro.observe(el)
    if (el.parentElement) ro.observe(el.parentElement)
    document.fonts?.addEventListener('loadingdone', fit)
    return () => {
      ro.disconnect()
      document.fonts?.removeEventListener('loadingdone', fit)
    }
  }, [text, lines])
  return (
    <div className="ti" ref={ref} title={text}>
      <span className="ti-in">
        {text}
        <span className="ti-dup" aria-hidden="true">
          {text}
        </span>
      </span>
    </div>
  )
}

/** Quality badge (hidden when the detector is off, see useBadge). */
export function Badge({ track }: { track: Track }) {
  const b = useBadge(track)
  if (!b) return null
  return (
    <span className={`q q-${b.kind}`} title={b.extra ? `${b.label} ${b.extra}` : b.label}>
      {b.tick && <Ico.Tick />}
      <span className="q-l">{b.label}</span>
      {b.extra && <small>{b.extra}</small>}
      {b.short !== b.label && <span className="q-s">{b.short}</span>}
    </span>
  )
}

// ---------- centre: transport ----------

function Transport() {
  const t = useT()
  const { status, shuffle, repeat, ab } = useStore((s) => s.player)
  const has = useStore((s) => !!s.track)
  const playing = status === 'playing'
  const repeatLabel = t(repeat === 'all' ? 'player.repeatAll' : repeat === 'one' ? 'player.repeatOne' : 'player.repeatOff')
  const abLabel = !ab ? t('shell.abSetA') : ab.b === null ? t('shell.abSetB', { a: time(ab.a) }) : t('shell.abOff', { a: time(ab.a), b: time(ab.b) })
  return (
    <div className="tr">
      {/* No stop button: stop stays on the media keys and the system media controls */}
      <div className="tr-main">
        <button className="tb2" aria-label={t('player.previous')} title={t('player.previous')} onClick={player.previous}>
          <Ico.Prev />
        </button>
        <button className="play" aria-label={t(playing ? 'player.pause' : 'player.play')} title={t(playing ? 'player.pause' : 'player.play')} onClick={player.toggle}>
          {playing ? <Ico.Pause /> : <Ico.Play />}
        </button>
        <button className="tb2" aria-label={t('player.next')} title={t('player.next')} onClick={player.next}>
          <Ico.Next />
        </button>
      </div>
      <div className="tr-sub">
        <button className={repeat !== 'off' ? 'on' : ''} aria-pressed={repeat !== 'off'} aria-label={repeatLabel} title={repeatLabel} onClick={player.cycleRepeat}>
          <Ico.Repeat one={repeat === 'one'} />
        </button>
        <button className={`ab ${ab ? 'on' : ''} ${ab && ab.b === null ? 'half' : ''}`} aria-pressed={!!ab} aria-label={abLabel} title={abLabel} disabled={!has} onClick={player.cycleAb}>
          {ab && ab.b === null ? 'A-' : 'A-B'}
        </button>
        <button className={shuffle ? 'on' : ''} aria-pressed={shuffle} aria-label={t('player.shuffle')} title={t('player.shuffle')} onClick={player.toggleShuffle}>
          <Ico.Shuffle />
        </button>
      </div>
    </div>
  )
}

// ---------- right: (mini: window buttons) + volume + tools ----------

function Side({ search }: { search: ReactNode }) {
  const t = useT()
  const view = useStore((s) => s.view)
  const mini = useStore((s) => s.mini)
  const onTop = useStore((s) => (s.mini ? s.prefs.miniOnTop : s.prefs.onTop))
  const vu = findSkin(useStore((s) => s.skin.id)).vu
  const settings = (section: 'eq' | 'remote') => () => void leaveMini().then(() => setState({ view: 'settings', section }))
  const pinLabel = t(onTop ? 'shell.onTopOff' : 'shell.onTopOn')
  return (
    <div className="side">
      {mini && <MiniBar search={search} />}
      <Volume />
      <div className="tools">
        {vu && <Vu />}
        {/* In mini, a quick native menu (on/off + presets) instead of leaving for Ajustes */}
        <button
          className="ib"
          aria-label={t('eq.title')}
          title={t('eq.title')}
          aria-haspopup={mini && native ? 'menu' : undefined}
          onClick={(e) => (mini && native ? void eqMenu(e.currentTarget) : settings('eq')())}
        >
          <Ico.Eq />
        </button>
        <button
          className={`ib ${view === 'lyrics' && !mini ? 'on' : ''}`}
          aria-pressed={view === 'lyrics' && !mini}
          aria-label={t('nav.lyrics')}
          title={t('nav.lyrics')}
          onClick={() => void leaveMini().then(() => setState({ view: view === 'lyrics' ? 'library' : 'lyrics' }))}
        >
          <Ico.Lyrics />
        </button>
        <SleepButton />
        <button className={`ib ${onTop ? 'on' : ''}`} aria-pressed={onTop} aria-label={pinLabel} title={pinLabel} onClick={() => setPrefs(mini ? { miniOnTop: !onTop } : { onTop: !onTop })}>
          <Ico.Pin on={onTop} />
        </button>
        {!mini && (
          <button className="ib" aria-label={t('settings.remote.title')} title={t('settings.remote.title')} onClick={settings('remote')}>
            <Ico.Phone />
          </button>
        )}
      </div>
    </div>
  )
}

/** Mini mode has no title bar: search, Ajustes, back to the full window, minimize, close. */
function MiniBar({ search }: { search: ReactNode }) {
  const t = useT()
  return (
    <div className="mbar">
      {search}
      <button className="wc" aria-label={t('nav.settings')} title={t('nav.settings')} onClick={() => void leaveMini().then(() => setState({ view: 'settings' }))}>
        <Ico.Gear />
      </button>
      <button className="wc" aria-label={t('shell.full')} title={`${t('shell.full')} (${isMac ? '⌘ M' : 'Ctrl M'})`} onClick={() => void toggleMini().catch(() => {})}>
        <Ico.Mini full />
      </button>
      <WindowControls mini />
    </div>
  )
}

/** Thin capsule that fills with the ink colour (no knob, no number): drag or click anywhere,
 *  wheel ±2 %, arrows. The speaker to its right shows the level and toggles mute. */
function Volume() {
  const t = useT()
  const v = useStore((s) => s.player.volume)
  const lock = useVolumeLock()
  const before = useRef(0.8)
  const muted = v === 0
  const pct = Math.round(v * 100)
  const wheel = (dy: number): void => {
    if (!lock && dy) player.setVolume(Math.max(0, Math.min(1, (pct + (dy < 0 ? 2 : -2)) / 100)))
  }
  return (
    <div className="vol" onWheel={(e) => wheel(e.deltaY)}>
      <Slider className="cap" value={v} onChange={player.setVolume} label={t('player.volume')} valueText={`${pct} %`} locked={lock} />
      <button
        className="spk"
        aria-label={t(muted ? 'player.unmute' : 'player.mute')}
        title={lock ?? t(muted ? 'player.unmute' : 'player.mute')}
        disabled={!!lock}
        onClick={() => {
          if (muted) player.setVolume(before.current || 0.8)
          else {
            before.current = v
            player.setVolume(0)
          }
        }}
      >
        <Ico.Volume level={v} />
      </button>
    </div>
  )
}

/** Mini's EQ button: on/off and the presets, ✓ on the current ones; the full editor in Ajustes. */
async function eqMenu(at: HTMLElement): Promise<void> {
  const st = await api.eqGet().catch(() => null)
  if (!st) return
  const { eq, preset } = st
  const set = (...a: Parameters<typeof api.eqSet>) => () => void api.eqSet(...a).catch(() => {})
  await popupMenu(
    [
      { text: tr('eq.title') },
      { text: tr('eq.on'), checked: eq.enabled, run: set({ ...eq, enabled: !eq.enabled }, preset) },
      null,
      ...st.presets.map((p) => ({
        text: p.builtin ? tr(`eq.preset.${p.name}`) : p.name,
        checked: preset === p.id,
        run: set({ ...p.eq, enabled: true, autoPreamp: eq.autoPreamp, preamp: eq.preamp }, p.id)
      })),
      null,
      { text: tr('shell.eqOpen'), run: () => void leaveMini().then(() => setState({ view: 'settings', section: 'eq' })) }
    ],
    at
  )
}

/** Pletina's little VU: four bands, drawn straight to the DOM. */
function Vu() {
  const bars = useRef<HTMLElement[]>([])
  const val = useRef([0, 0, 0, 0])
  useSpectrum(4, (frame) => {
    for (let i = 0; i < 4; i++) {
      const target = frame ? frame[i] / 255 : 0
      val.current[i] += (target - val.current[i]) * (target > val.current[i] ? 0.5 : 0.15)
      const el = bars.current[i]
      if (el) el.style.transform = `scaleY(${Math.max(0.08, val.current[i]).toFixed(3)})`
    }
  })
  return (
    <div className="vu" aria-hidden="true">
      {[0, 1, 2, 3].map((i) => (
        <i key={i} ref={(el) => void (el && (bars.current[i] = el))} />
      ))}
    </div>
  )
}

const SLEEP = [15, 30, 45, 60, 90] as const

/** Sleep timer: a small menu under the clock button. The timer runs in the backend. */
function SleepButton() {
  const t = useT()
  const sleep = useStore((s) => s.player.sleep)
  const [open, setOpen] = useState(false)
  const btn = useRef<HTMLButtonElement>(null)
  const left = useMinutesLeft(sleep?.endsAt ?? null)
  const close = useCallback(() => setOpen(false), [])
  const label = !sleep ? t('shell.sleep') : sleep.endOfTrack ? t('shell.sleepTrackOn') : t('shell.sleepIn', { n: left })
  // Inside the app a native menu (it can leave the 150 px mini window); the browser preview keeps
  // the in-page one
  const nativeMenu = (at: HTMLElement): void =>
    void popupMenu(
      [
        { text: sleep ? `${t('shell.sleep')} · ${label}` : t('shell.sleep') },
        { text: t('shell.sleepOff'), checked: !sleep, run: () => player.sleep(null) },
        ...SLEEP.map((m) => ({ text: t('shell.sleepMinutes', { n: m }), checked: false, run: () => player.sleep(m) })),
        null,
        { text: t('shell.sleepTrack'), checked: !!sleep?.endOfTrack, run: () => player.sleep('track') }
      ],
      at
    )
  return (
    <>
      <button
        ref={btn}
        className={`ib ${sleep ? 'on' : ''}`}
        aria-label={label}
        title={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => (native ? nativeMenu(e.currentTarget) : setOpen((o) => !o))}
      >
        <Ico.Timer />
        {sleep?.endsAt && <small className="ib-n">{left}</small>}
      </button>
      {open && <SleepMenu anchor={btn.current} status={label} onClose={close} />}
    </>
  )
}

function useMinutesLeft(endsAt: number | null): number {
  const [, bump] = useState(0)
  useEffect(() => {
    if (!endsAt) return
    const id = setInterval(() => bump((n) => n + 1), 10_000)
    return () => clearInterval(id)
  }, [endsAt])
  return endsAt ? Math.max(1, Math.ceil((endsAt - Date.now()) / 60_000)) : 0
}

function SleepMenu({ anchor, status, onClose }: { anchor: HTMLElement | null; status: string; onClose: () => void }) {
  const t = useT()
  const sleep = useStore((s) => s.player.sleep)
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const a = anchor?.getBoundingClientRect()
    const m = ref.current?.getBoundingClientRect()
    if (!a || !m) return
    const below = a.bottom + 6 + m.height <= innerHeight - 6
    setPos({ left: Math.max(6, Math.min(a.right - m.width, innerWidth - m.width - 6)), top: below ? a.bottom + 6 : Math.max(6, a.top - m.height - 6) })
    ref.current?.querySelector<HTMLElement>('[aria-checked=true], [role=menuitemradio]')?.focus()
  }, [anchor])
  useEffect(() => {
    const away = (e: Event): void => {
      if (!ref.current?.contains(e.target as Node) && !anchor?.contains(e.target as Node)) onClose()
    }
    window.addEventListener('pointerdown', away, true)
    window.addEventListener('resize', onClose)
    return () => {
      window.removeEventListener('pointerdown', away, true)
      window.removeEventListener('resize', onClose)
      anchor?.focus({ preventScroll: true })
    }
  }, [anchor, onClose])
  const pick = (v: number | 'track' | null) => () => {
    player.sleep(v)
    onClose()
  }
  const onKey = (e: KeyboardEvent): void => {
    const items = [...(ref.current?.querySelectorAll<HTMLElement>('[role=menuitemradio]') ?? [])]
    const i = items.indexOf(document.activeElement as HTMLElement)
    if (e.key === 'Escape') onClose()
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') items[(i + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus()
    else if (e.key === 'Tab') onClose()
    else return
    e.preventDefault()
    e.stopPropagation()
  }
  const item = (key: string, label: string, on: boolean, run: () => void) => (
    <button key={key} role="menuitemradio" aria-checked={on} className={`tmenu-i ${on ? 'on' : ''}`} onClick={run}>
      <span className="tmenu-ck">{on && <Ico.Tick />}</span>
      {label}
    </button>
  )
  return (
    <div ref={ref} className="tmenu pop" role="menu" aria-label={t('shell.sleep')} style={{ left: pos?.left ?? 0, top: pos?.top ?? 0, visibility: pos ? 'visible' : 'hidden' }} onKeyDown={onKey}>
      <div className="tmenu-h">
        <b>{t('shell.sleep')}</b>
        <span>{sleep ? status : t('shell.sleepHint')}</span>
      </div>
      {item('off', t('shell.sleepOff'), !sleep, pick(null))}
      {SLEEP.map((m) => item(String(m), t('shell.sleepMinutes', { n: m }), false, pick(m)))}
      {item('track', t('shell.sleepTrack'), !!sleep?.endOfTrack, pick('track'))}
    </div>
  )
}

// ---------- bottom: waveform seekbar ----------

/** CSS px per waveform column (AIMP-dense). No gaps between columns: at 1-2 px a gap turns any
 *  sub-pixel offset of the bar into moiré bands. */
const COL_PX = 2
/** Peak halo around the RMS body: its opacity, and how far past the body it may reach (brickwalled
 *  masters peak near full scale everywhere; uncapped, the halo would be a flat band hiding the shape) */
const HALO = 0.35
const HALO_REACH = 1.4
/** Height the loudest stretch of the track reaches, and a very mild curve so quiet parts stay visible */
const BODY_MAX = 0.92
const BODY_CURVE = 0.9

/** Device pixels of the seekbar, and the ratio to CSS px */
interface Box {
  w: number
  h: number
  dpr: number
}

/** The waveform as an alpha mask (a PNG data URL), so its colours stay plain CSS tokens that follow
 *  the skin and the cover palette. Mirrored around the centre line, one column per COL_PX. The body
 *  is the RMS scaled to the track's loudest stretch (modern masters have flat peaks, the RMS shows the
 *  intro, verses, choruses and breaks); the peaks are a faint fringe around it. No data = a flat line. */
function waveMask({ w, h, dpr }: Box, data: Uint8Array | null): string {
  const c = document.createElement('canvas')
  c.width = w
  c.height = h
  const g = c.getContext('2d')
  if (!g || !c.width || !c.height) return 'none'
  const mid = c.height / 2
  const line = Math.max(1, Math.round(dpr))
  if (!data || data.length < 2) {
    g.globalAlpha = 0.6
    g.fillRect(0, mid - line / 2, c.width, line)
    return `url(${c.toDataURL()})`
  }
  const n = data.length >> 1
  let loudest = 1
  for (let i = n; i < 2 * n; i++) loudest = Math.max(loudest, data[i])
  const cols = Math.max(1, Math.floor(w / dpr / COL_PX))
  for (let i = 0; i < cols; i++) {
    const a = Math.floor((i * n) / cols)
    const b = Math.max(a + 1, Math.floor(((i + 1) * n) / cols))
    let p = 0
    let r = 0
    for (let k = a; k < b; k++) {
      p = Math.max(p, data[k])
      r = Math.max(r, data[n + k])
    }
    const peak = p / 255
    const body = Math.min(peak, BODY_MAX * Math.pow(r / loudest, BODY_CURVE))
    const x = Math.round((i * c.width) / cols)
    const bw = Math.round(((i + 1) * c.width) / cols) - x
    const bar = (v: number): void => {
      const half = Math.max(line / 2, v * mid)
      const top = Math.round(mid - half)
      g.fillRect(x, top, bw, Math.round(mid + half) - top)
    }
    g.globalAlpha = HALO
    bar(Math.min(peak, body * HALO_REACH))
    g.globalAlpha = 1
    bar(body)
  }
  return `url(${c.toDataURL()})`
}

function Wave({ track }: { track: Track | null }) {
  const t = useT()
  const { secs, duration } = usePosition()
  const box = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState<Box>({ w: 0, h: 0, dpr: 1 })
  useLayoutEffect(() => {
    const el = box.current
    if (!el) return
    // Size in device pixels, exact where the browser says (Chromium), so the mask maps 1:1
    const ro = new ResizeObserver(([e]) => {
      const dpr = window.devicePixelRatio || 1
      const d = e.devicePixelContentBoxSize?.[0]
      setSize({ w: d?.inlineSize ?? Math.round(e.contentRect.width * dpr), h: d?.blockSize ?? Math.round(e.contentRect.height * dpr), dpr })
    })
    try {
      ro.observe(el, { box: 'device-pixel-content-box' })
    } catch {
      ro.observe(el)
    }
    return () => ro.disconnect()
  }, [])
  const data = useWaveform(track?.id ?? null)
  // Redrawn only when the data or the size changes; the played part is a CSS clip on --p
  const mask = useMemo(() => (size.w && size.h ? waveMask(size, data) : 'none'), [size, data])
  const layer: CSSProperties = { maskImage: mask, WebkitMaskImage: mask }
  const ab = useStore((s) => s.player.ab)
  return (
    <div className="wv">
      <span className="wv-t">{time(secs)}</span>
      <div className="wv-box" ref={box}>
        <Seekbar className="wave" value={secs} max={duration} onSeek={player.seek} label={t('player.position')} valueText={`${time(secs)} / ${time(duration)}`}>
          <div className={`wv-l ${data ? 'in' : ''}`} key={data ? `w${track?.id}` : 'flat'} style={layer} />
          <div className={`wv-l fill ${data ? 'in' : ''}`} key={data ? `f${track?.id}` : 'flatf'} style={layer} />
        </Seekbar>
        {ab && duration > 0 && (
          <i className="wv-ab" aria-hidden="true" style={{ left: `${(ab.a / duration) * 100}%`, width: ab.b === null ? 2 : `${((ab.b - ab.a) / duration) * 100}%` }} />
        )}
      </div>
      <span className="wv-t">{time(duration)}</span>
    </div>
  )
}
