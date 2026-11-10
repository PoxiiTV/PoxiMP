// Letras: lyrics of the playing track. Synced ones follow the song (current line highlighted and
// centred; scrolling by hand pauses that for a moment) and a click on a line seeks there.
import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../../core/api'
import { time } from '../../core/format'
import { player, usePosition } from '../../core/hooks'
import { useT } from '../../core/i18n'
import { useStore } from '../../core/store'
import type { LyricLine, Lyrics as LyricsData } from '../../core/types'
import { Empty, ViewHead } from './common'

/** Hand scrolling pauses the auto-scroll this long after the last wheel / key / touch. */
const RESUME_AFTER_MS = 3500
/** Lines light up this much early: reading is a little ahead of hearing. */
const LEAD_MS = 150

type Load = { id: number; status: 'loading' } | { id: number; status: 'ok'; data: LyricsData } | { id: number; status: 'offline' }

export function Lyrics() {
  const t = useT()
  const track = useStore((s) => s.track)
  const [load, setLoad] = useState<Load | null>(null)
  const [online, setOnline] = useState<boolean | null>(null)
  const [retry, setRetry] = useState(0)
  const id = track?.id

  useEffect(() => {
    api.lyricsOnline().then(setOnline, () => {})
  }, [])

  useEffect(() => {
    if (id === undefined) return
    let alive = true
    setLoad({ id, status: 'loading' })
    api.lyrics(id).then(
      (data) => alive && setLoad({ id, status: 'ok', data }),
      (e) => alive && setLoad(e === 'network_error' ? { id, status: 'offline' } : { id, status: 'ok', data: { synced: null, plain: null, source: null } })
    )
    return () => {
      alive = false
    }
  }, [id, retry])

  if (!track) return <div className="vw vw-lyrics"><Empty title={t('player.idle')} hint={t('views.lyrics.idleHint')} /></div>

  const cur = load?.id === track.id ? load : null
  const data = cur?.status === 'ok' ? cur.data : null
  const enable = (): void => {
    api.setLyricsOnline(true).then((on) => {
      setOnline(on)
      setRetry((r) => r + 1)
    }, () => {})
  }

  return (
    <div className="vw vw-lyrics">
      <ViewHead
        kicker={
          <>
            {t('views.lyrics.kicker')}
            {data?.source && <> · {t(`views.lyrics.source.${data.source}`)}</>}
            {data && !data.synced && data.plain && <> · {t('views.lyrics.plain')}</>}
          </>
        }
        title={track.title}
        meta={<span>{track.artist}</span>}
      />
      {!cur || cur.status === 'loading' ? (
        <p className="vw-hint vw-ly-wait" role="status">
          {t('views.lyrics.loading')}
        </p>
      ) : cur.status === 'offline' ? (
        <Empty title={t('views.lyrics.offline')} hint={t('views.lyrics.offlineHint')}>
          <button className="btn" onClick={() => setRetry((r) => r + 1)}>
            {t('views.lyrics.retry')}
          </button>
        </Empty>
      ) : data?.synced ? (
        <Synced key={track.id} lines={data.synced} />
      ) : data?.plain ? (
        <div className="vw-scrollpane vw-ly-plain">
          {data.plain.split('\n').map((l, i) => (
            <p key={i} className={l.trim() ? '' : 'gap'}>
              {l}
            </p>
          ))}
        </div>
      ) : (
        <Empty title={t('views.lyrics.none')} hint={online === false ? t('views.lyrics.noneOffHint') : t('views.lyrics.noneHint')}>
          {online === false && (
            <button className="btn" onClick={enable}>
              {t('views.lyrics.enableOnline')}
            </button>
          )}
        </Empty>
      )}
    </div>
  )
}

/** Index of the line being sung (-1 before the first), by binary search. */
function lineAt(lines: LyricLine[], ms: number): number {
  let lo = 0
  let hi = lines.length - 1
  let at = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (lines[mid].tMs <= ms) {
      at = mid
      lo = mid + 1
    } else hi = mid - 1
  }
  return at
}

/** Renders nothing: follows the position every frame and reports only line changes. */
function Tracker({ lines, onLine }: { lines: LyricLine[]; onLine: (i: number) => void }) {
  const { secs } = usePosition()
  const i = lineAt(lines, secs * 1000 + LEAD_MS)
  useEffect(() => onLine(i), [i, onLine])
  return null
}

function Synced({ lines }: { lines: LyricLine[] }) {
  const t = useT()
  const box = useRef<HTMLDivElement>(null)
  const [line, setLine] = useState(-1)
  const [follow, setFollow] = useState(true)
  const timer = useRef(0)

  const centre = useCallback((i: number, smooth: boolean) => {
    const el = box.current
    const row = el?.querySelector<HTMLElement>(`[data-i="${Math.max(0, i)}"]`)
    if (!el || !row) return
    const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches
    el.scrollTo({ top: row.offsetTop - el.clientHeight / 2 + row.offsetHeight / 2, behavior: smooth && !reduce ? 'smooth' : 'auto' })
  }, [])

  useEffect(() => {
    if (follow) centre(line, true)
  }, [line, follow, centre])
  useEffect(() => () => clearTimeout(timer.current), [])

  // Only real hand input pauses following (the smooth scroll above fires scroll events too)
  const byHand = (): void => {
    setFollow(false)
    clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setFollow(true), RESUME_AFTER_MS)
  }
  const resume = (): void => {
    clearTimeout(timer.current)
    setFollow(true)
  }

  return (
    <div className="vw-ly">
      <Tracker lines={lines} onLine={setLine} />
      <div
        ref={box}
        className="vw-scrollpane vw-ly-lines"
        onWheel={byHand}
        onTouchMove={byHand}
        onPointerDown={(e) => e.target === e.currentTarget && byHand()}
        onKeyDown={(e) => ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End'].includes(e.key) && byHand()}
      >
        {lines.map((l, i) => (
          <button
            key={i}
            data-i={i}
            className={`vw-ly-l ${i === line ? 'on' : i < line ? 'past' : ''} ${l.text ? '' : 'gap'}`}
            aria-current={i === line || undefined}
            title={t('views.lyrics.seek', { time: time(l.tMs / 1000) })}
            onClick={() => {
              player.seek(l.tMs / 1000)
              resume()
            }}
          >
            {l.text || '♪'}
          </button>
        ))}
      </div>
      {!follow && (
        <button className="btn sm vw-ly-follow" onClick={resume}>
          {t('views.lyrics.follow')}
        </button>
      )}
    </div>
  )
}
