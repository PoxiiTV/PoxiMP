// Ajustes take the whole window under the title bar: sections on the left (with a small
// now-playing card), the open section on the right. Esc or "Volver al reproductor" goes back.
import { useEffect, useRef, type ComponentType } from 'react'
import { player } from '../../core/hooks'
import { useT } from '../../core/i18n'
import { SECTIONS, setState, useStore, type Section } from '../../core/store'
import { EffectsPage } from '../Effects'
import { EqPage, OutputPage, PlaybackPage } from '../Equalizer'
import { AboutSection, AppearanceSection, GeneralSection, ImportSection, KeysSection, LibrarySection, LyricsSection, QualitySection, RemoteSection } from '../Settings'
import * as Ico from './icons'
import { Cover } from './Player'

const PAGES: Record<Section, ComponentType> = {
  general: GeneralSection,
  appearance: AppearanceSection,
  playback: PlaybackPage,
  output: OutputPage,
  eq: EqPage,
  fx: EffectsPage,
  library: LibrarySection,
  quality: QualitySection,
  lyrics: LyricsSection,
  remote: RemoteSection,
  keys: KeysSection,
  import: ImportSection,
  about: AboutSection
}
/** A line goes before these */
const GROUPS: Section[] = ['quality', 'keys']

export function SettingsScreen() {
  const t = useT()
  const section = useStore((s) => s.section)
  const pane = useRef<HTMLDivElement>(null)
  const Page = PAGES[section]
  useEffect(() => {
    pane.current?.scrollTo(0, 0)
  }, [section])
  return (
    <div className="set">
      <nav className="nav" aria-label={t('settings.title')}>
        <button className="back" onClick={() => setState({ view: 'library' })}>
          <Ico.Back />
          {t('shell.backToPlayer')}
        </button>
        <div className="nav-list">
          {SECTIONS.map((id) => {
            const Icon = Ico.SECTION[id]
            return (
              <div key={id} className="nav-i">
                {GROUPS.includes(id) && <i className="sep" aria-hidden="true" />}
                <button data-s={id} className={`it ${id === section ? 'on' : ''}`} aria-current={id === section ? 'page' : undefined} onClick={() => setState({ section: id })}>
                  <Icon />
                  <span>{t(`set.${id}.title`)}</span>
                </button>
              </div>
            )
          })}
        </div>
        <NowCard />
      </nav>
      <div className="pane" ref={pane}>
        <div className="pane-in settings">
          <h2>{t(`set.${section}.title`)}</h2>
          <p className="pane-d">{t(`set.${section}.desc`)}</p>
          <Page />
        </div>
      </div>
    </div>
  )
}

function NowCard() {
  const t = useT()
  const track = useStore((s) => s.track)
  const playing = useStore((s) => s.player.status === 'playing')
  return (
    <div className="np">
      <Cover track={track} />
      <div className="np-t">
        <b>{track?.title ?? t('player.idle')}</b>
        <span>{track?.artist ?? ''}</span>
      </div>
      <button className="play" aria-label={t(playing ? 'player.pause' : 'player.play')} onClick={player.toggle}>
        {playing ? <Ico.Pause /> : <Ico.Play />}
      </button>
    </div>
  )
}
