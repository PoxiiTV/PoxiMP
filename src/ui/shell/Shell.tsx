// The one layout every skin shares (docs/bocetos/06-estructura.html): title bar → player panel →
// tabs → content → status bar. Mini mode is the same window shrunk to the panel alone (it carries
// its own window buttons, docs/bocetos/08-mini.html); Ajustes takes the whole window below the
// title bar. Skins only theme it (src/skins/<id>/theme.css).
import './shell.css'
import { useEffect, useRef, type CSSProperties } from 'react'
import { coverUrl } from '../../core/api'
import { num, total } from '../../core/format'
import { reloadLibrary } from '../../core/hooks'
import { useT } from '../../core/i18n'
import { usePalette, type Palette } from '../../core/palette'
import { NAV, setState, useStore, type View } from '../../core/store'
import type { Track } from '../../core/types'
import { toggleMini, useWindowMode } from '../../core/window'
import { findSkin } from '../../skins/registry'
import { player } from '../../core/hooks'
import { useGlobalSearch } from '../Search'
import { UpdateDialogs } from '../Updater'
import { TrackMenu } from '../TrackMenu'
import { SharedView } from '../views'
import { WindowControls, dragRegion, isMac } from '../WindowControls'
import * as Ico from './icons'
import { Library, LibraryActions } from './Library'
import { Badge, PlayerPanel } from './Player'
import { SettingsScreen } from './SettingsScreen'

type Css = CSSProperties & Record<`--${string}`, string>

export function Shell() {
  const skin = useStore((s) => s.skin)
  const view = useStore((s) => s.view)
  const mini = useStore((s) => s.mini)
  const analyze = useStore((s) => s.qset.analyze)
  const track = useStore((s) => s.track)
  const cover = useCoverTint(findSkin(skin.id).cover ? track : null)
  useWindowMode()
  useShortcuts()
  useSearchFilter()
  const settings = view === 'settings' && !mini
  return (
    <div className={`app ${mini ? 'mini' : ''} ${settings ? 'settings' : ''} ${analyze ? '' : 'noq'}`} style={cover}>
      {!mini && <TitleBar />}
      {settings ? (
        <SettingsScreen />
      ) : (
        <>
          <PlayerPanel />
          {!mini && (
            <>
              <Tabs />
              <main className="body">{view === 'library' ? <Library /> : <div className="view"><SharedView view={view} badge={Badge} /></div>}</main>
            </>
          )}
        </>
      )}
      {!mini && <StatusBar />}
      <TrackMenu />
      <UpdateDialogs />
    </div>
  )
}

// ---------- title bar ----------

function TitleBar() {
  const t = useT()
  const view = useStore((s) => s.view)
  const { input, popup } = useGlobalSearch()
  const openSettings = (): void => setState({ view: view === 'settings' ? 'library' : 'settings' })
  return (
    <header className="tb" {...dragRegion}>
      <div className="logo" {...dragRegion}>
        <i aria-hidden="true" />
        PoxiMP
      </div>
      <label className="search">
        <Ico.Search />
        <input {...input} placeholder={t('search.placeholder')} aria-label={t('library.search')} spellCheck={false} />
        {input.value ? (
          <button className="x" aria-label={t('library.clearSearch')} onClick={() => setState({ search: '' })}>
            <Ico.Close />
          </button>
        ) : (
          <kbd>{isMac ? '⌘ K' : 'Ctrl K'}</kbd>
        )}
      </label>
      {popup}
      <div className="tb-r" {...dragRegion}>
        <button className={`ib ${view === 'settings' ? 'on' : ''}`} aria-pressed={view === 'settings'} aria-label={t('nav.settings')} title={t('nav.settings')} onClick={openSettings}>
          <Ico.Gear />
        </button>
        <button className="ib" aria-label={t('shell.mini')} title={`${t('shell.mini')} (${isMac ? '⌘ M' : 'Ctrl M'})`} onClick={() => void toggleMini().catch(() => {})}>
          <Ico.Mini />
        </button>
        <WindowControls />
      </div>
    </header>
  )
}

// ---------- tabs ----------

function Tabs() {
  const t = useT()
  const view = useStore((s) => s.view)
  const current = (v: View): boolean => view === v || (v === 'stats' && view === 'wrapped')
  return (
    <nav className="tabs" aria-label={t('shell.tabs')}>
      {NAV.map((v) => {
        const Icon = Ico.TAB[v]
        return (
          <button key={v} className={`tab ${current(v) ? 'on' : ''}`} aria-current={current(v) ? 'page' : undefined} onClick={() => setState({ view: v })}>
            <Icon />
            <span>{t(`nav.${v}`)}</span>
          </button>
        )
      })}
      <span className="sp" />
      {view === 'library' && <LibraryActions />}
    </nav>
  )
}

// ---------- status bar ----------

function StatusBar() {
  const t = useT()
  const totals = useStore((s) => s.totals)
  const scan = useStore((s) => s.scan)
  const analyze = useStore((s) => s.qset.analyze)
  const out = useStore((s) => s.output)
  const n = totals?.total ?? 0
  const pending = totals?.counts.pending ?? 0
  return (
    <footer className="st">
      {scan ? (
        <span>{t('library.scanning', { done: scan.done, total: scan.total })}</span>
      ) : (
        <span>
          <b>{n.toLocaleString()}</b> {n === 1 ? t('shell.songOne') : t('shell.songs')} · {total(totals?.duration ?? 0)}
        </span>
      )}
      {analyze && pending > 0 && <span className="st-q">{t('shell.analyzing', { done: (n - pending).toLocaleString(), total: n.toLocaleString() })}</span>}
      <span className="sp" />
      {out && out.sampleRate > 0 && (
        <span className="st-out" title={out.device}>
          <i className={`dot ${out.fallback && out.fallback !== 'formatChanged' ? 'warn' : ''}`} />
          <span className="st-dev">{out.device}</span>
          <span>
            {' · '}
            <b>{out.bitPerfect ? t('player.bitPerfect') : out.exclusive ? t('shell.exclusive') : t('shell.shared')}</b> {num(out.sampleRate / 1000)} kHz
            {out.bits ? ` / ${out.bits} bit` : ''}
          </span>
        </span>
      )}
    </footer>
  )
}

// ---------- behaviour ----------

/** Space = play/pause, Ctrl/⌘ M = mini, Esc = out of Ajustes. */
function useShortcuts(): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      const el = e.target as HTMLElement
      const typing = !!el.closest('input, textarea, select, [contenteditable=true]')
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'm') {
        e.preventDefault()
        void toggleMini().catch(() => {})
      } else if (e.code === 'Space' && !typing && !el.closest('button, [role=slider], [role=radio], [role=switch], [role=menuitem], [role=option]')) {
        e.preventDefault()
        player.toggle()
      } else if (e.key === 'Escape' && !e.defaultPrevented && !typing && useStore.getState().view === 'settings') {
        setState({ view: 'library' })
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}

/** The search text also filters Biblioteca (debounced), whatever tab is open. */
function useSearchFilter(): void {
  const q = useStore((s) => s.search)
  useEffect(() => {
    if (q === useStore.getState().library.filter) return
    const id = setTimeout(() => reloadLibrary({ filter: q }), 160)
    return () => clearTimeout(id)
  }, [q])
}

/** Cara A: colours from the cover (lightness fixed so contrast holds for any cover). */
function useCoverTint(track: Track | null): Css | undefined {
  const loaded = usePalette(track ? coverUrl(track.id) : null)
  // Keep the previous colours while the next cover loads (no flash to the default palette)
  const last = useRef<Palette | null>(null)
  if (loaded !== undefined) last.current = loaded
  const p = track ? last.current : null
  if (!p) return undefined
  const c = (h: number, s: number, l: number): string => `hsl(${Math.round(h)} ${Math.round(s)}% ${l}%)`
  const b = p.base.h
  const bs = Math.min(45, Math.max(18, p.base.s * 100))
  const a = p.accent.h
  const as = Math.min(78, Math.max(45, p.accent.s * 100))
  return {
    '--bg': c(b, bs, 8.6),
    '--chrome': c(b, bs * 0.95, 6.2),
    '--pane': c(b, bs * 0.95, 7.4),
    '--card': c(b, bs * 0.92, 11.5),
    '--field': c(b, bs * 0.92, 12.5),
    '--deck-a': c(b, bs, 16),
    '--deck-b': c(b, bs, 9.6),
    '--acc': c(a + 10, Math.min(90, as + 13), 66),
    '--ink2': c(a, 28, 76),
    '--ink3': c(b, 16, 58)
  }
}
