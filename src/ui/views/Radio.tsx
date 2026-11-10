// Radio y mezclas: a radio from what is playing (or the selected track), and the automatic mixes.
import { api } from '../../core/api'
import { errorText, useT } from '../../core/i18n'
import { useStore } from '../../core/store'
import { toast } from '../TrackMenu'
import { ViewHead } from './common'
import { Mixes } from './Mixes'

export function Radio() {
  const t = useT()
  // An opened file (outside the library) cannot seed a radio: its candidates are library tracks.
  const seed = useStore((s) => (s.track && !s.track.external ? s.track : s.selected?.external ? null : s.selected))
  const start = (): void => {
    if (seed) api.radioStart(seed.id).then(() => toast(t('views.menu.radioDone', { title: seed.title })), (e) => toast(errorText(e)))
  }
  return (
    <div className="vw vw-radio">
      <ViewHead kicker={t('views.radio.kicker')} title={t('nav.radio')} />
      <div className="vw-scrollpane">
        <section className="vw-sec">
          <h3 className="vw-sec-t">{t('views.radio.title')}</h3>
          <p className="vw-hint">{seed ? t('views.radio.hint', { title: seed.title, artist: seed.artist }) : t('views.radio.none')}</p>
          <div className="set-actions">
            <button className="btn primary" disabled={!seed} onClick={start}>
              {t('views.radio.start')}
            </button>
          </div>
        </section>
        <Mixes />
      </div>
    </div>
  )
}
