// Biblioteca: artists · tracks · information (details + quality report of the selected track).
import { Fragment, memo, useCallback, useEffect, useMemo, useState, type KeyboardEvent } from 'react'
import { api } from '../../core/api'
import { currentLang, useT } from '../../core/i18n'
import { formatLabel, num, time } from '../../core/format'
import { addFolder, player, reloadLibrary, useLibrary, useQuality } from '../../core/hooks'
import { setState, useStore } from '../../core/store'
import type { Artist, SortKey, Track } from '../../core/types'
import { detail, rich, type Detail } from '../quality'
import { isMenuKey, openTrackMenu } from '../TrackMenu'
import { VirtualList } from '../VirtualList'
import * as Ico from './icons'
import { Badge } from './Player'

const ROW_H = 34
const ARTIST_H = 30
const SORTS: SortKey[] = ['artist', 'title', 'album', 'duration', 'added']

export function Library() {
  const t = useT()
  const lib = useLibrary()
  // Empty library: one centred call to action over the whole area (no empty artists column / info panel)
  if (lib.ready && lib.total === 0 && !lib.filter && lib.artist === null)
    return (
      <div className="lib lib-empty">
        <div className="lib-empty-ico" aria-hidden="true">
          <Ico.TAB.library />
        </div>
        <h3>{t('library.empty')}</h3>
        <p>{t('library.emptyHint')}</p>
        <button className="btn primary" onClick={() => void addFolder().catch(() => {})}>
          <Ico.Plus />
          {t('library.addFolder')}
        </button>
      </div>
    )
  return (
    <div className="lib">
      <Artists />
      <Tracks />
      <Info />
    </div>
  )
}

/** Right side of the tab bar while Biblioteca is open. */
export function LibraryActions() {
  const t = useT()
  const sort = useStore((s) => s.library.sort)
  return (
    <div className="act">
      <label className="pill sel">
        <span>{t('shell.sortBy')}</span>
        <select value={sort} aria-label={t('shell.sortBy')} onChange={(e) => reloadLibrary({ sort: e.target.value as SortKey })}>
          {SORTS.map((k) => (
            <option key={k} value={k}>
              {t(`shell.sort.${k}`)}
            </option>
          ))}
        </select>
      </label>
      <button className="pill" aria-label={t('library.addFolder')} title={t('library.addFolder')} onClick={() => void addFolder().catch(() => {})}>
        <Ico.Plus />
        <span>{t('library.addFolder')}</span>
      </button>
    </div>
  )
}

// ---------- artists ----------

function Artists() {
  const t = useT()
  const pick = useStore((s) => s.library.artist)
  const all = useStore((s) => s.totals?.total ?? 0)
  const scan = useStore((s) => s.scan)
  const [list, setList] = useState<Artist[]>([])
  useEffect(() => {
    if (!scan) api.artists().then(setList, () => {})
  }, [scan, all])
  const choose = (id: number | null): void => {
    if (id !== useStore.getState().library.artist) reloadLibrary({ artist: id })
  }
  const active = pick === null ? 0 : list.findIndex((a) => a.id === pick) + 1
  const onKey = (e: KeyboardEvent): void => {
    const d = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0
    if (!d) return
    e.preventDefault()
    const n = Math.max(0, Math.min(list.length, active + d))
    choose(n === 0 ? null : list[n - 1].id)
  }
  return (
    <div className="col">
      <h4 id="lib-artists">{t('shell.artists')}</h4>
      <VirtualList
        className="col-list"
        count={list.length + 1}
        rowHeight={ARTIST_H}
        active={active}
        role="listbox"
        tabIndex={0}
        aria-labelledby="lib-artists"
        aria-activedescendant={`lib-a${active}`}
        onKeyDown={onKey}
      >
        {(i) => {
          const a = i === 0 ? null : list[i - 1]
          return (
            <div id={`lib-a${i}`} role="option" aria-selected={i === active} className={`li ${i === active ? 'on' : ''}`} onClick={() => choose(a ? a.id : null)}>
              <span>{a ? a.name : t('shell.allArtists')}</span>
              <em>{(a ? a.trackCount : all).toLocaleString()}</em>
            </div>
          )
        }}
      </VirtualList>
    </div>
  )
}

// ---------- tracks ----------

function Tracks() {
  const t = useT()
  const lib = useLibrary()
  const current = useStore((s) => s.track)
  const selected = useStore((s) => s.selected)
  const { ensureRange } = lib
  const selIndex = selected ? lib.rows.findIndex((r) => r?.id === selected.id) : -1
  const select = useCallback((tr: Track) => setState({ selected: tr }), [])

  const onKey = (e: KeyboardEvent): void => {
    const d = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0
    if (d) {
      e.preventDefault()
      const r = lib.rows[Math.max(0, Math.min(lib.total - 1, selIndex + d))]
      if (r) select(r)
    } else if (e.key === 'Enter' && selIndex >= 0) player.playRow(selIndex)
    else if (isMenuKey(e) && selected && selIndex >= 0) {
      e.preventDefault()
      const el = document.getElementById(`lib-r${selIndex}`)
      if (el) openTrackMenu(el, selected, { onPlay: () => player.playRow(selIndex) })
    }
  }


  return (
    <div className="tbl">
      <div className="row h" aria-hidden="true">
        <span className="n">{t('library.cols.n')}</span>
        <span>{t('library.cols.title')}</span>
        <span>{t('library.cols.artist')}</span>
        <span className="al">{t('views.cols.album')}</span>
        <span className="d">{t('library.cols.duration')}</span>
        <span className="f">{t('library.cols.format')}</span>
        <span className="qcol">{t('library.cols.quality')}</span>
      </div>
      {lib.ready && lib.total === 0 ? (
        <p className="none">{t('library.noResults', { q: lib.filter })}</p>
      ) : (
        <VirtualList
          className="rows"
          count={lib.total}
          rowHeight={ROW_H}
          onRange={ensureRange}
          active={selIndex}
          role="listbox"
          tabIndex={0}
          aria-label={t('nav.library')}
          aria-activedescendant={selIndex >= 0 ? `lib-r${selIndex}` : undefined}
          onKeyDown={onKey}
        >
          {(i) => <Row i={i} track={lib.rows[i]} sel={i === selIndex} playing={!!current && lib.rows[i]?.id === current.id} onSelect={select} />}
        </VirtualList>
      )}
    </div>
  )
}

const Row = memo(function Row({ i, track, sel, playing, onSelect }: { i: number; track?: Track; sel: boolean; playing: boolean; onSelect: (t: Track) => void }) {
  if (!track)
    return (
      <div className="row skel" aria-hidden="true">
        <span />
        <span>
          <i />
        </span>
      </div>
    )
  return (
    <div
      id={`lib-r${i}`}
      role="option"
      aria-selected={sel}
      aria-current={playing || undefined}
      className={`row ${playing ? 'on' : ''} ${sel ? 'sel' : ''}`}
      onClick={() => onSelect(track)}
      onDoubleClick={() => player.playRow(i)}
      onContextMenu={(e) => {
        onSelect(track)
        openTrackMenu(e, track, { onPlay: () => player.playRow(i) })
      }}
    >
      <span className="n">{playing ? <Ico.Play /> : String(i + 1).padStart(2, '0')}</span>
      <span className="tt">{track.title}</span>
      <span className="a">{track.artist}</span>
      <span className="a al">{track.album}</span>
      <span className="d">{time(track.duration)}</span>
      <span className="f">{formatLabel(track)}</span>
      <span className="qcol">
        <Badge track={track} />
      </span>
    </div>
  )
})

// ---------- information ----------

/** "ayer", "hace 3 días", "12 mar 2024" */
function when(unix: number | null, never: string): string {
  if (!unix) return never
  const days = Math.round((new Date().setHours(0, 0, 0, 0) - new Date(unix * 1000).setHours(0, 0, 0, 0)) / 86_400_000)
  const lang = currentLang() === 'es' ? 'es-ES' : 'en-GB'
  if (days < 30) return new Intl.RelativeTimeFormat(lang, { numeric: 'auto' }).format(-days, 'day')
  return new Date(unix * 1000).toLocaleDateString(lang, { day: 'numeric', month: 'short', year: 'numeric' })
}

function Info() {
  const t = useT()
  const track = useStore((s) => s.selected ?? s.track)
  const analyze = useStore((s) => s.qset.analyze)
  const dash = '—'
  const rows: [string, string][] = track
    ? [
        [t('views.cols.album'), track.album || dash],
        [t('shell.info.year'), track.year ? String(track.year) : dash],
        [t('shell.info.genre'), track.genre || dash],
        [t('shell.info.format'), formatLabel(track)],
        [t('shell.info.plays'), String(track.playCount)],
        [t('shell.info.last'), when(track.lastPlayedAt, t('shell.info.never'))],
        ['ReplayGain', track.replayGain === null ? dash : `${track.replayGain < 0 ? '−' : '+'}${num(Math.abs(track.replayGain))} dB`]
      ]
    : []
  return (
    <aside className="info" aria-label={t('shell.info.title')}>
      <h4>{t('shell.info.title')}</h4>
      {track ? (
        <>
          <p className="info-t" title={`${track.title} · ${track.artist}`}>
            <b>{track.title}</b>
            <span>{track.artist}</span>
          </p>
          <dl className="kv">
            {rows.map(([k, v]) => (
              <Fragment key={k}>
                <dt>{k}</dt>
                <dd title={v}>{v}</dd>
              </Fragment>
            ))}
          </dl>
          {analyze && <QualityBox track={track} />}
        </>
      ) : (
        <p className="info-none">{t('shell.info.none')}</p>
      )}
    </aside>
  )
}

function QualityBox({ track }: { track: Track }) {
  const t = useT()
  const q = useQuality(track)
  const d = detail(track, q)
  return (
    <>
      <h4>{t('library.cols.quality')}</h4>
      <div className="qbox" aria-live="polite">
        <Badge track={track} />
        <p>
          {rich(d.text).map((p, i) => (
            <Fragment key={i}>{p.bold ? <strong>{p.text}</strong> : p.text}</Fragment>
          ))}
        </p>
        <Sketch s={d.sketch} />
        <dl className="facts">
          {d.facts.map((f) => (
            <div key={f.label}>
              <dt>{f.label}</dt>
              <dd className={f.tone ?? ''}>{f.value}</dd>
            </div>
          ))}
        </dl>
      </div>
    </>
  )
}

/** Where the sound ends and what is missing (hatched), on a 0..Nyquist axis. */
function Sketch({ s }: { s: Detail['sketch'] }) {
  const t = useT()
  const W = 240
  /** Under the cut line, but never over the "0" or the max label (8.5 px mono: up to 6.2 units a char, Ruido's is the widest) */
  const cutX = (cx: number, label: string, max: string): number => {
    const half = (label.length * 6.2) / 2
    return Math.min(Math.max(cx, 6 + 10 + half), W - 6 - max.length * 6.2 - 6 - half)
  }
  const top = 6
  const base = 50
  const shape = useMemo(() => {
    if (!s) return null
    const cx = Math.round((Math.min(s.cut, s.max) / s.max) * (W - 12)) + 6
    let d = `M6 ${base}`
    let seed = 7
    for (let x = 6; x <= cx; x += 3) {
      seed = (seed * 9301 + 49297) % 233280
      const y = top + 5 + Math.pow((x - 6) / (W - 12), 0.8) * 30 + (seed / 233280) * 5
      d += ` L${x} ${Math.min(base - 2, y).toFixed(1)}`
    }
    return { cx, d: `${d} L${cx} ${base} Z`, full: s.cut >= s.max - 0.05 }
  }, [s])
  return (
    <svg className="spec" viewBox={`0 0 ${W} 62`} aria-hidden="true">
      <defs>
        <pattern id="spec-hatch" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
          <path d="M0 0v5" className="sk-hatch" />
        </pattern>
      </defs>
      {s && shape ? (
        <>
          {!shape.full && <rect x={shape.cx} y={top} width={W - 6 - shape.cx} height={base - top} fill="url(#spec-hatch)" />}
          <path d={shape.d} className={`sk-area ${shape.full ? 'ok' : 'bad'}`} />
          {!shape.full && <path d={`M${shape.cx} ${top - 2}V${base}`} className="sk-cut" />}
          <path d={`M6 ${base}H${W - 6}`} className="sk-base" />
          <g className="sk-axis">
            <text x="6" y="59">
              0
            </text>
            {!shape.full && (
              <text x={cutX(shape.cx, `${num(s.cut)} kHz`, `${num(s.max)} kHz`)} y="59" textAnchor="middle">
                {num(s.cut)} kHz
              </text>
            )}
            <text x={W - 6} y="59" textAnchor="end">
              {num(s.max)} kHz
            </text>
          </g>
        </>
      ) : (
        <text x={W / 2} y="34" textAnchor="middle" className="sk-axis">
          {t('quality.noData')}
        </text>
      )}
    </svg>
  )
}
