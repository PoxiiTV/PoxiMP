// Wrapped: a year in music as a story — a sequence of big-number slides with keyboard/arrow/click
// navigation. Colours and fonts come from the skin through the --v-* hooks; motion is CSS only and
// drops to a plain fade with prefers-reduced-motion.
import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react'
import { api } from '../../core/api'
import { currentLang, useT } from '../../core/i18n'
import { setState } from '../../core/store'
import type { TopTrack, Wrapped as WrappedT } from '../../core/types'
import * as Ico from '../shell/icons'
import { QualityBadge } from './common'
import { Bars, Cover, minutes } from './Stats'

type Css = CSSProperties & Record<`--${string}`, string | number>
const YEARS = 5
const reduced = (): boolean => matchMedia('(prefers-reduced-motion: reduce)').matches
const fmtN = (n: number): string => Math.round(n).toLocaleString(currentLang() === 'es' ? 'es-ES' : 'en-GB')

/** Counts up to `to` when the slide appears (instant with reduced motion). */
function CountUp({ to, fmt = fmtN, ms = 1100 }: { to: number; fmt?: (n: number) => string; ms?: number }) {
  const [v, setV] = useState(() => (reduced() ? to : 0))
  useEffect(() => {
    if (reduced()) return setV(to)
    let id = 0
    const t0 = performance.now()
    const tick = (now: number): void => {
      const p = Math.min(1, (now - t0) / ms)
      setV(to * (1 - Math.pow(1 - p, 3)))
      if (p < 1) id = requestAnimationFrame(tick)
    }
    id = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(id)
  }, [to, ms])
  return <span className="wr-count">{fmt(v)}</span>
}

interface Slide {
  id: string
  mood: number
  body: ReactNode
}

export function Wrapped() {
  const t = useT()
  const thisYear = new Date().getFullYear()
  const [year, setYear] = useState(thisYear)
  const [w, setW] = useState<WrappedT | null>(null)
  const [i, setI] = useState(0)
  const [dir, setDir] = useState(1)
  const root = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let alive = true
    setW(null)
    setI(0)
    api.wrapped(year).then((x) => alive && setW(x), () => alive && setW(null))
    return () => {
      alive = false
    }
  }, [year])

  const slides = useMemo(() => (w ? build(w, t) : []), [w, t])
  const n = slides.length
  const goTo = (k: number): void => {
    const next = Math.max(0, Math.min(n - 1, k))
    if (next === i) return
    setDir(next > i ? 1 : -1)
    setI(next)
  }
  const exit = (): void => setState({ view: 'stats' })

  useEffect(() => {
    root.current?.focus({ preventScroll: true })
  }, [])

  const onKey = (e: KeyboardEvent): void => {
    if ((e.target as HTMLElement).tagName === 'SELECT') return
    if (e.key === 'ArrowRight' || e.key === 'PageDown' || (e.key === ' ' && e.target === root.current)) goTo(i + 1)
    else if (e.key === 'ArrowLeft' || e.key === 'PageUp') goTo(i - 1)
    else if (e.key === 'Home') goTo(0)
    else if (e.key === 'End') goTo(n - 1)
    else if (e.key === 'Escape') exit()
    else return
    e.preventDefault()
    e.stopPropagation()
  }

  const s = slides[i]
  return (
    <div ref={root} className="vw vw-wrapped" tabIndex={-1} onKeyDown={onKey} aria-roledescription={t('views.wrapped.story')} aria-label={t('views.wrapped.title', { year })}>
      <div className={`wr-stage mood-${s?.mood ?? 0}`}>
        <div className="wr-bg" aria-hidden="true">
          <i />
          <i />
          <i />
        </div>
        <header className="wr-top">
          <div className="wr-progress" role="group" aria-label={t('views.wrapped.slides')}>
            {slides.map((x, k) => (
              <button key={x.id} aria-current={k === i ? 'step' : undefined} aria-label={t('views.wrapped.slideN', { i: k + 1, n })} className={k < i ? 'done' : k === i ? 'on' : ''} onClick={() => goTo(k)}>
                <i />
              </button>
            ))}
          </div>
          <div className="wr-tools">
            <label className="wr-year">
              <span className="vw-sr">{t('views.wrapped.year')}</span>
              <select value={year} onChange={(e) => setYear(Number(e.target.value))}>
                {Array.from({ length: YEARS }, (_, k) => thisYear - k).map((y) => (
                  <option key={y} value={y}>
                    {y}
                  </option>
                ))}
              </select>
            </label>
            <button className="wr-close" aria-label={t('views.wrapped.close')} title={t('views.wrapped.close')} onClick={exit}>
              <Ico.Close />
            </button>
          </div>
        </header>

        {!w ? (
          <div className="wr-slide wr-wait" />
        ) : w.stats.plays === 0 ? (
          <div className="wr-slide wr-empty">
            <p className="wr-k">{t('views.wrapped.kicker')}</p>
            <h2 className="wr-huge">{year}</h2>
            <p className="wr-lead">{t('views.wrapped.none')}</p>
          </div>
        ) : (
          s && (
            <div key={`${year}-${s.id}`} className={`wr-slide s-${s.id}`} style={{ '--dir': dir } as Css} aria-live="polite">
              {s.body}
            </div>
          )
        )}

        {n > 1 && w && w.stats.plays > 0 && (
          <>
            <button className="wr-hit wr-hit-prev" tabIndex={-1} aria-hidden="true" disabled={i === 0} onClick={() => goTo(i - 1)} />
            <button className="wr-hit wr-hit-next" tabIndex={-1} aria-hidden="true" disabled={i === n - 1} onClick={() => goTo(i + 1)} />
            <nav className="wr-nav" aria-label={t('views.wrapped.slides')}>
              <button className="wr-arrow" disabled={i === 0} aria-label={t('views.wrapped.prev')} onClick={() => goTo(i - 1)}>
                <Ico.ChevronLeft />
              </button>
              <span className="wr-pos" aria-hidden="true">
                {i + 1} / {n}
              </span>
              {i < n - 1 ? (
                <button className="wr-arrow" aria-label={t('views.wrapped.next')} onClick={() => goTo(i + 1)}>
                  <Ico.ChevronRight />
                </button>
              ) : (
                <button className="wr-again" onClick={() => goTo(0)}>
                  {t('views.wrapped.again')}
                </button>
              )}
            </nav>
          </>
        )}
      </div>
    </div>
  )
}

function Rest({ rows, from = 1 }: { rows: { key: string | number; name: string; sub?: string; n: number }[]; from?: number }) {
  const t = useT()
  if (!rows.length) return null
  return (
    <ol className="wr-rest">
      {rows.map((r, k) => (
        <li key={r.key} style={{ '--k': k } as Css}>
          <span className="wr-rest-i">{k + from + 1}</span>
          <span className="wr-rest-t">
            <b>{r.name}</b>
            {r.sub && <span>{r.sub}</span>}
          </span>
          <span className="wr-rest-n">{t('views.stats.playsN', { n: r.n })}</span>
        </li>
      ))}
    </ol>
  )
}

const trackRows = (rows: TopTrack[]) => rows.map((r) => ({ key: r.track.id, name: r.track.title, sub: r.track.artist, n: r.plays }))

function build(w: WrappedT, t: (k: string, p?: Record<string, string | number>) => string): Slide[] {
  const st = w.stats
  const out: Slide[] = []
  const months = t('views.wrapped.months').split(',')

  out.push({
    id: 'intro',
    mood: 0,
    body: (
      <>
        <p className="wr-k">{t('views.wrapped.kicker')}</p>
        <h2 className="wr-huge year">{w.year}</h2>
        <p className="wr-lead">{t('views.wrapped.minutesLead')}</p>
        <p className="wr-big">
          <CountUp to={Math.round(st.listenedMs / 60000)} />
          <small>{t('views.wrapped.minutes')}</small>
        </p>
        <p className="wr-sub">{t('views.wrapped.introSub', { plays: fmtN(st.plays), tracks: fmtN(st.tracks) })}</p>
      </>
    )
  })

  const artist = st.topArtists[0]
  if (artist)
    out.push({
      id: 'artist',
      mood: 1,
      body: (
        <div className="wr-two">
          <div className="wr-art">
            <Cover id={artist.coverId} className="wr-cover round" />
          </div>
          <div>
            <p className="wr-k">{t('views.wrapped.artistK')}</p>
            <h2 className="wr-name">{artist.name}</h2>
            <p className="wr-sub">{t('views.wrapped.artistSub', { n: fmtN(artist.plays), m: minutes(artist.listenedMs) })}</p>
            <Rest rows={st.topArtists.slice(1, 5).map((a) => ({ key: a.name, name: a.name, n: a.plays }))} />
          </div>
        </div>
      )
    })

  const top = st.topTracks[0]
  if (top)
    out.push({
      id: 'track',
      mood: 2,
      body: (
        <div className="wr-two">
          <div className="wr-art">
            <Cover id={top.track.id} className="wr-cover" />
          </div>
          <div>
            <p className="wr-k">{t('views.wrapped.trackK')}</p>
            <h2 className="wr-name">{top.track.title}</h2>
            <p className="wr-by">{top.track.artist}</p>
            <p className="wr-big sm">
              <CountUp to={top.plays} ms={900} />
              <small>{t('views.wrapped.times')}</small>
            </p>
            <Rest rows={trackRows(st.topTracks.slice(1, 4))} />
          </div>
        </div>
      )
    })

  if (w.lateNight)
    out.push({
      id: 'night',
      mood: 3,
      body: (
        <div className="wr-two">
          <div className="wr-art">
            <div className="wr-moon" aria-hidden="true" />
            <Cover id={w.lateNight.track.id} className="wr-cover sm" />
          </div>
          <div>
            <p className="wr-k">{t('views.wrapped.nightK')}</p>
            <h2 className="wr-name">{w.lateNight.track.title}</h2>
            <p className="wr-by">{w.lateNight.track.artist}</p>
            <p className="wr-sub">{t('views.wrapped.nightSub', { n: w.lateNight.plays })}</p>
          </div>
        </div>
      )
    })

  if (w.topMonth) {
    const m = w.topMonth.month - 1
    out.push({
      id: 'month',
      mood: 4,
      body: (
        <>
          <p className="wr-k">{t('views.wrapped.monthK')}</p>
          <h2 className="wr-huge">{months[m]}</h2>
          <p className="wr-sub">{t('views.wrapped.monthSub', { n: fmtN(w.topMonth.plays), m: minutes(w.topMonth.listenedMs) })}</p>
          <Bars
            className="wr-months"
            values={w.byMonth}
            mark={m}
            labels={months.map((x) => x.slice(0, 1))}
            titles={w.byMonth.map((ms, k) => t('views.stats.barDay', { d: months[k], n: minutes(ms) }))}
          />
        </>
      )
    })
  }

  out.push({
    id: 'quality',
    mood: 5,
    body: (
      <>
        <p className="wr-k">{t('views.wrapped.qualityK')}</p>
        <h2 className="wr-title">{w.topVerified.length ? t('views.wrapped.qualityT') : t('views.wrapped.qualityNone')}</h2>
        {w.topVerified.length > 0 ? (
          <ol className="wr-verified">
            {w.topVerified.slice(0, 5).map((r, k) => (
              <li key={r.track.id} style={{ '--k': k } as Css}>
                <span className="wr-rest-i">{k + 1}</span>
                <Cover id={r.track.id} />
                <span className="wr-rest-t">
                  <b>{r.track.title}</b>
                  <span>{r.track.artist}</span>
                </span>
                <QualityBadge track={r.track} />
              </li>
            ))}
          </ol>
        ) : (
          <p className="wr-sub">{t('views.wrapped.qualityHint')}</p>
        )}
      </>
    )
  })

  out.push({
    id: 'library',
    mood: 6,
    body: (
      <>
        <p className="wr-k">{t('views.wrapped.libraryK')}</p>
        <p className="wr-huge pct">
          <CountUp to={w.libraryPct} fmt={(v) => `${Math.round(v)}`} />
          <small>%</small>
        </p>
        <p className="wr-lead">{t('views.wrapped.libraryLead')}</p>
        <div className="wr-facts">
          <span>
            <b>
              <CountUp to={st.discoveries} ms={800} />
            </b>
            {t('views.wrapped.discoveries')}
          </span>
          <span>
            <b>
              <CountUp to={st.streak.days} ms={800} />
            </b>
            {t('views.wrapped.streak')}
          </span>
        </div>
      </>
    )
  })
  return out
}
