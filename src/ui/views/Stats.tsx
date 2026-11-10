// Estadísticas: range tabs, totals, tops, hour-of-day and weekday bars (plain CSS), discoveries, streak.
// Wrapped is one click away.
import { useEffect, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react'
import { api, coverUrl } from '../../core/api'
import { num, total as fmtTotal } from '../../core/format'
import { currentLang, useT } from '../../core/i18n'
import { setState } from '../../core/store'
import type { Stats as StatsT, StatsRange, TopItem, TopTrack } from '../../core/types'
import * as Ico from '../shell/icons'
import { openTrackMenu } from '../TrackMenu'
import { Empty, ViewHead } from './common'

const RANGES: StatsRange[] = ['week', 'month', 'year', 'all']
type Css = CSSProperties & Record<`--${string}`, string | number>

/** Remembered while the app runs, so going to Wrapped and back keeps the tab */
let lastRange: StatsRange = 'month'

export const minutes = (ms: number): string => Math.round(ms / 60000).toLocaleString(currentLang() === 'es' ? 'es-ES' : 'en-GB')
const longDate = (ymd: string | null): string =>
  ymd ? new Date(`${ymd}T12:00:00`).toLocaleDateString(currentLang() === 'es' ? 'es-ES' : 'en-GB', { day: 'numeric', month: 'short' }) : ''

export function Cover({ id, className = '' }: { id: number | null; className?: string }) {
  const [bad, setBad] = useState(false)
  return <span className={`vw-cover ${className}`}>{id !== null && !bad && <img src={coverUrl(id)} alt="" draggable={false} onError={() => setBad(true)} />}</span>
}

export function Stats() {
  const t = useT()
  const [range, setRange] = useState<StatsRange>(lastRange)
  const [st, setSt] = useState<StatsT | null>(null)
  const [error, setError] = useState(false)
  useEffect(() => {
    let alive = true
    lastRange = range
    setError(false)
    api.stats(range).then((s) => alive && setSt(s), () => alive && setError(true))
    return () => {
      alive = false
    }
  }, [range])

  const onTabKey = (e: KeyboardEvent): void => {
    const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0
    if (!d) return
    e.preventDefault()
    const next = RANGES[(RANGES.indexOf(range) + d + RANGES.length) % RANGES.length]
    setRange(next)
    requestAnimationFrame(() => document.getElementById(`vw-range-${next}`)?.focus())
  }
  const year = new Date().getFullYear()

  return (
    <div className="vw vw-stats">
      <ViewHead kicker={t('views.stats.kicker')} title={t('nav.stats')}>
        <button className="btn primary vw-wrapped-btn" onClick={() => setState({ view: 'wrapped' })}>
          <Ico.Sparkle />
          {t('views.stats.wrapped', { year })}
        </button>
      </ViewHead>
      <div className="vw-tabs" role="tablist" aria-label={t('views.stats.range')} onKeyDown={onTabKey}>
        {RANGES.map((r) => (
          <button key={r} id={`vw-range-${r}`} role="tab" aria-selected={r === range} aria-controls="vw-stats-panel" tabIndex={r === range ? 0 : -1} className={r === range ? 'on' : ''} onClick={() => setRange(r)}>
            {t(`views.stats.ranges.${r}`)}
          </button>
        ))}
      </div>
      <div id="vw-stats-panel" role="tabpanel" aria-labelledby={`vw-range-${range}`} className="vw-scrollpane" aria-busy={!st}>
        {error ? (
          <Empty title={t('err.unknown')} />
        ) : !st ? null : st.plays === 0 ? (
          <Empty title={t('views.stats.none')} hint={t('views.stats.noneHint')} />
        ) : (
          <Body st={st} />
        )}
      </div>
    </div>
  )
}

function Body({ st }: { st: StatsT }) {
  const t = useT()
  const hours = st.listenedMs / 3600000
  const peakHour = st.byHour.indexOf(Math.max(...st.byHour))
  const days = t('views.stats.weekdays').split(',')
  const peakDay = st.byWeekday.indexOf(Math.max(...st.byWeekday))
  return (
    <>
      <div className="vw-tiles">
        <Tile big={hours >= 10 ? num(hours, 0) : num(hours, 1)} unit={t('views.stats.hours')} label={t('views.stats.listened')} sub={t('views.stats.minutes', { n: minutes(st.listenedMs) })} accent />
        <Tile big={st.plays.toLocaleString(currentLang() === 'es' ? 'es-ES' : 'en-GB')} label={t('views.stats.plays')} />
        <Tile big={String(st.tracks)} label={t('views.stats.tracks')} />
        <Tile big={String(st.discoveries)} label={t('views.stats.discoveries')} />
        <Tile big={String(st.streak.days)} unit={st.streak.days === 1 ? t('views.stats.day') : t('views.stats.days')} label={t('views.stats.streak')} sub={st.streak.from ? `${longDate(st.streak.from)} – ${longDate(st.streak.to)}` : ''} />
      </div>

      <div className="vw-grid">
        <Panel title={t('views.stats.topTracks')}>
          <TopTracks rows={st.topTracks} />
        </Panel>
        <Panel title={t('views.stats.topArtists')}>
          <TopList rows={st.topArtists} round />
        </Panel>
        <Panel title={t('views.stats.topAlbums')}>
          <TopList rows={st.topAlbums} />
        </Panel>
        <Panel title={t('views.stats.topGenres')}>
          <TopList rows={st.topGenres} noCover />
        </Panel>
        <Panel title={t('views.stats.byHour')} note={t('views.stats.peakHour', { h: peakHour })}>
          <Bars values={st.byHour} labels={st.byHour.map((_, h) => (h % 6 === 0 ? String(h) : ''))} titles={st.byHour.map((ms, h) => t('views.stats.barHour', { h, n: minutes(ms) }))} className="hours" />
        </Panel>
        <Panel title={t('views.stats.byWeekday')} note={t('views.stats.peakDay', { d: days[peakDay] ?? '' })}>
          <Bars values={st.byWeekday} labels={days.map((d) => d.slice(0, 3))} titles={st.byWeekday.map((ms, i) => t('views.stats.barDay', { d: days[i] ?? '', n: minutes(ms) }))} className="days" />
        </Panel>
        {st.topDiscoveries.length > 0 && (
          <Panel title={t('views.stats.topDiscoveries')} wide>
            <TopTracks rows={st.topDiscoveries.slice(0, 5)} />
          </Panel>
        )}
      </div>
    </>
  )
}

function Tile({ big, unit, label, sub, accent }: { big: string; unit?: string; label: string; sub?: string; accent?: boolean }) {
  return (
    <div className={`vw-tile ${accent ? 'accent' : ''}`}>
      <span className="vw-tile-l">{label}</span>
      <span className="vw-tile-n">
        {big}
        {unit && <small> {unit}</small>}
      </span>
      {sub && <span className="vw-tile-s">{sub}</span>}
    </div>
  )
}

function Panel({ title, note, wide, children }: { title: string; note?: string; wide?: boolean; children: ReactNode }) {
  return (
    <section className={`vw-panel ${wide ? 'wide' : ''}`}>
      <header>
        <h3 className="vw-sec-t">{title}</h3>
        {note && <span className="vw-note">{note}</span>}
      </header>
      {children}
    </section>
  )
}

function TopTracks({ rows }: { rows: TopTrack[] }) {
  const t = useT()
  const max = rows[0]?.plays || 1
  return (
    <ol className="vw-top">
      {rows.map((r, i) => (
        <li
          key={r.track.id}
          style={{ '--w': r.plays / max } as Css}
          onDoubleClick={() => void api.playTrack(r.track.id).catch(() => {})}
          onContextMenu={(e) => openTrackMenu(e, r.track)}
          title={t('views.stats.dblPlay')}
        >
          <span className="vw-rank">{i + 1}</span>
          <Cover id={r.track.id} />
          <span className="vw-top-t">
            <b>{r.track.title}</b>
            <span>{r.track.artist}</span>
          </span>
          <span className="vw-top-n">{t('views.stats.playsN', { n: r.plays })}</span>
        </li>
      ))}
    </ol>
  )
}

function TopList({ rows, round, noCover }: { rows: TopItem[]; round?: boolean; noCover?: boolean }) {
  const t = useT()
  const max = rows[0]?.plays || 1
  return (
    <ol className={`vw-top ${round ? 'round' : ''} ${noCover ? 'flat' : ''}`}>
      {rows.slice(0, 8).map((r, i) => (
        <li key={`${r.name}${r.key ?? ''}`} style={{ '--w': r.plays / max } as Css}>
          <span className="vw-rank">{i + 1}</span>
          {!noCover && <Cover id={r.coverId} />}
          <span className="vw-top-t">
            <b>{r.name}</b>
            {r.by !== undefined && r.key !== null && <span>{r.by ?? t('views.stats.various')}</span>}
            {noCover && <span>{fmtTotal(r.listenedMs / 1000)}</span>}
          </span>
          <span className="vw-top-n">{t('views.stats.playsN', { n: r.plays })}</span>
        </li>
      ))}
    </ol>
  )
}

/** Vertical bars, heights relative to the highest; the highest is marked. */
export function Bars({ values, labels, titles, className = '', mark }: { values: number[]; labels: string[]; titles: string[]; className?: string; mark?: number }) {
  const max = Math.max(1, ...values)
  const top = mark ?? values.indexOf(Math.max(...values))
  return (
    <div className={`vw-bars ${className}`} role="img" aria-label={titles.filter((_, i) => values[i] > 0).join(', ')}>
      {values.map((v, i) => (
        <span key={i} className={`vw-bar ${i === top ? 'top' : ''}`} title={titles[i]} style={{ '--h': v / max } as Css}>
          <i />
          <em>{labels[i]}</em>
        </span>
      ))}
    </div>
  )
}
