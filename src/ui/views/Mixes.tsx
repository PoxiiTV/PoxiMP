// "Mezclas para ti" (top of Listas): automatic mixes from what is played most and how it sounds,
// computed offline by the backend. A tile plays its mix.
import { useEffect, useState } from 'react'
import { api, coverUrl } from '../../core/api'
import { total as fmtTotal } from '../../core/format'
import { errorText, useT } from '../../core/i18n'
import type { Mix } from '../../core/types'
import { toast } from '../TrackMenu'

export function Mixes() {
  const t = useT()
  const [mixes, setMixes] = useState<Mix[] | null>(null)
  useEffect(() => {
    api.mixes().then(setMixes, () => setMixes([]))
  }, [])

  const name = (m: Mix, i: number): string => (m.genre ? t('views.mixes.named', { name: m.genre }) : t('views.mixes.numbered', { n: i + 1 }))
  const play = (m: Mix, i: number): void => {
    api.playQueue(m.trackIds, 0).then(() => toast(t('views.mixes.playing', { name: name(m, i) })), (e) => toast(errorText(e)))
  }

  return (
    <section className="vw-sec">
      <h3 className="vw-sec-t">{t('views.mixes.title')}</h3>
      <p className="vw-hint">{mixes && !mixes.length ? t('views.mixes.empty') : t('views.mixes.hint')}</p>
      {mixes && mixes.length > 0 && (
        <ul className="vw-cards vw-mixes">
          {mixes.map((m, i) => (
            <li key={i}>
              <button className="vw-card vw-mix" aria-label={t('views.mixes.play', { name: name(m, i) })} onClick={() => play(m, i)}>
                <span className={`vw-mix-art n${Math.min(4, m.coverIds.length)}`} aria-hidden="true">
                  {m.coverIds.slice(0, 4).map((id) => (
                    <img key={id} src={coverUrl(id)} alt="" draggable={false} onError={(e) => (e.currentTarget.style.visibility = 'hidden')} />
                  ))}
                </span>
                <span className="vw-card-t vw-mix-t">
                  <b>{name(m, i)}</b>
                  <span>{m.artists.length ? t('views.mixes.and', { names: m.artists.join(', ') }) : ''}</span>
                  <small>
                    {t('library.songs', { n: m.trackIds.length })} · {fmtTotal(m.durationMs / 1000)}
                  </small>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
