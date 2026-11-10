// App updates, the PoxiLauncher way: on open and every 10 min the app asks GitHub for the latest
// release (latest.json, signed with the key in .env). Newer = "PoxiMP X disponible" with
// [Más tarde] [Actualizar ahora] → download progress → the installer runs on its own (passive) and
// opens PoxiMP again. From the portable the same installer installs PoxiMP on the PC. The first
// launch after an update shows "Novedades", read from CHANGELOG.md (bundled at build time).
import { useEffect } from 'react'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { create } from 'zustand'
import changelog from '../../CHANGELOG.md?raw'
import { changesSince, isNewer, type ChangeItem } from '../core/changelog'
import { api, native } from '../core/api'
import { currentLang, useT } from '../core/i18n'
import { useStore } from '../core/store'
import './updater.css'

const EVERY_MS = 10 * 60_000
const SEEN_KEY = 'poximp.seenVersion'

type Upd = { state: 'available' | 'downloading' | 'error'; version: string; progress: number }
const useUpd = create<{ upd: Upd | null; news: { version: string; items: ChangeItem[] } | null }>(() => ({ upd: null, news: null }))

let pending: Update | null = null
/** Version whose notice already showed this session ("Más tarde" does not bring it back) */
let shown = ''

/** Asks for a newer version. `manual` = the Ajustes button: shows it again even after "Más tarde".
 *  Never in mini mode on its own (no room for the dialog: the next round asks again). */
export async function offerUpdate(manual = false): Promise<'none' | 'available' | 'error'> {
  if (!native) return 'none'
  if (useUpd.getState().upd) return 'available'
  if (!manual && useStore.getState().mini) return 'none'
  try {
    const u = await check({ timeout: 15_000 })
    if (!u) return 'none'
    if (!manual && u.version === shown) return 'none'
    shown = u.version
    pending = u
    useUpd.setState({ upd: { state: 'available', version: u.version, progress: 0 } })
    return 'available'
  } catch {
    return 'error'
  }
}

async function install(): Promise<void> {
  const u = pending
  const cur = useUpd.getState().upd
  if (!u || !cur) return
  useUpd.setState({ upd: { ...cur, state: 'downloading', progress: 0 } })
  let total = 0
  let done = 0
  let last = 0
  try {
    await u.downloadAndInstall((e) => {
      if (e.event === 'Started') total = e.data.contentLength ?? 0
      else if (e.event === 'Progress') {
        done += e.data.chunkLength
        if (total && performance.now() - last > 120) {
          last = performance.now()
          useUpd.setState({ upd: { ...cur, state: 'downloading', progress: Math.min(1, done / total) } })
        }
      }
    })
    // Windows: the installer has already closed the app and opens it again when done
    await relaunch()
  } catch {
    useUpd.setState({ upd: { ...cur, state: 'error', progress: 0 } })
  }
}

/** Startup: "Novedades" if this is the first launch of a newer version, then the update check. */
export function useUpdates(): void {
  useEffect(() => {
    if (!native) return
    void api.version().then((v) => {
      const seen = localStorage.getItem(SEEN_KEY)
      if (seen && isNewer(v, seen)) {
        const items = changesSince(changelog, seen, v, currentLang())
        if (items.length) useUpd.setState({ news: { version: v, items } })
      }
      if (seen !== v) localStorage.setItem(SEEN_KEY, v)
    }, () => {})
    void offerUpdate()
    const iv = setInterval(() => void offerUpdate(), EVERY_MS)
    return () => clearInterval(iv)
  }, [])
}

export function UpdateDialogs() {
  const t = useT()
  const { upd, news } = useUpd()
  const mini = useStore((s) => s.mini)
  if (mini) return null
  if (news) {
    const close = (): void => useUpd.setState({ news: null })
    return (
      <div className="upd-back" onClick={close}>
        <div className="upd-card upd-news" role="dialog" aria-modal="true" aria-labelledby="upd-news-t" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.key === 'Escape' && close()}>
          <h2 id="upd-news-t">{t('update.newsTitle', { v: news.version })}</h2>
          <ul className="upd-list">
            {news.items.map((it, i) => (
              <li key={i} style={{ animationDelay: `${100 + i * 50}ms` }}>
                <span className="upd-emoji" aria-hidden="true">
                  {it.emoji}
                </span>
                <div>
                  <b>{it.title}</b>
                  {it.text && <p>{it.text}</p>}
                </div>
              </li>
            ))}
          </ul>
          <div className="upd-actions">
            <button className="btn primary" autoFocus onClick={close}>
              {t('update.newsOk')}
            </button>
          </div>
        </div>
      </div>
    )
  }
  if (!upd) return null
  const close = (): void => useUpd.setState({ upd: null })
  const downloading = upd.state === 'downloading'
  return (
    <div className="upd-back" onClick={downloading ? undefined : close}>
      <div className="upd-card" role="dialog" aria-modal="true" aria-labelledby="upd-t" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => !downloading && e.key === 'Escape' && close()}>
        <h2 id="upd-t">{t('update.title', { v: upd.version })}</h2>
        {upd.state === 'available' && <p>{t('update.text')}</p>}
        {upd.state === 'error' && <p className="upd-err">{t('update.error')}</p>}
        {downloading && (
          <div className="upd-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(upd.progress * 100)}>
            <div className={`upd-bar ${upd.progress ? '' : 'busy'}`}>
              <i style={{ transform: `scaleX(${upd.progress || 1})` }} />
            </div>
            <span>{t('update.downloading', { p: Math.round(upd.progress * 100) })}</span>
          </div>
        )}
        {!downloading && (
          <div className="upd-actions">
            <button className="btn" onClick={close}>
              {upd.state === 'error' ? t('update.close') : t('update.later')}
            </button>
            {upd.state === 'available' && (
              <button className="btn primary" autoFocus onClick={() => void install()}>
                {t('update.now')}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
