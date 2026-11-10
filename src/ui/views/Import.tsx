// Migration: import AIMP / iTunes / M3U, stats backup in and out. Used in Ajustes (ImportSection)
// and, for playlists only, on the Listas index (ImportButtons).
import { useState } from 'react'
import { api } from '../../core/api'
import { errorText, useT } from '../../core/i18n'
import { setState } from '../../core/store'
import type { ImportReport } from '../../core/types'
import * as Ico from '../shell/icons'
import { pickFile, saveFile } from './common'

type Kind = 'aimp' | 'itunes' | 'm3u' | 'backupIn' | 'backupOut'

const ACTIONS: Record<Kind, { ext: string[]; name: string; run: (path: string) => Promise<ImportReport | number> }> = {
  aimp: { ext: ['aimppl4'], name: 'AIMP', run: api.importAimpPlaylist },
  itunes: { ext: ['xml'], name: 'iTunes Library.xml', run: api.importItunesXml },
  m3u: { ext: ['m3u', 'm3u8'], name: 'M3U', run: api.importM3u },
  backupIn: { ext: ['json'], name: 'PoxiMP', run: api.importStatsBackup },
  backupOut: { ext: ['json'], name: 'PoxiMP', run: api.exportStatsBackup }
}

interface Result {
  kind: Kind
  report?: ImportReport
  written?: number
  error?: string
}

export function ImportButtons({ only, onDone }: { only: Kind[]; onDone?: () => void }) {
  const t = useT()
  const [busy, setBusy] = useState<Kind | null>(null)
  const [res, setRes] = useState<Result | null>(null)

  const go = async (kind: Kind): Promise<void> => {
    const a = ACTIONS[kind]
    const path = kind === 'backupOut' ? await saveFile(a.name, a.ext, `poximp-${new Date().toISOString().slice(0, 10)}.json`) : await pickFile(a.name, a.ext)
    if (!path) return
    setBusy(kind)
    setRes(null)
    try {
      const r = await a.run(path)
      setRes(typeof r === 'number' ? { kind, written: r } : { kind, report: r })
      onDone?.()
    } catch (e) {
      setRes({ kind, error: errorText(e) })
    } finally {
      setBusy(null)
    }
  }

  return (
    <>
      <div className="set-actions vw-import">
        {only.map((k) => (
          <button key={k} className="btn" disabled={!!busy} aria-busy={busy === k} onClick={() => void go(k)}>
            {busy === k ? t('views.import.working') : t(`views.import.${k}`)}
          </button>
        ))}
      </div>
      {res && <Report res={res} onClose={() => setRes(null)} />}
    </>
  )
}

function Report({ res, onClose }: { res: Result; onClose: () => void }) {
  const t = useT()
  const r = res.report
  return (
    <div className={`vw-report ${res.error ? 'err' : ''}`} role="status">
      <div className="vw-report-h">
        <b>{t(`views.import.${res.kind}`)}</b>
        <button className="vw-x" aria-label={t('quality.closeReport')} onClick={onClose}>
          <Ico.Close />
        </button>
      </div>
      {res.error && <p className="warn">{res.error}</p>}
      {res.written !== undefined && <p>{t('views.import.written', { n: res.written })}</p>}
      {r && (
        <>
          <div className="vw-report-n">
            <span className="ok">
              <b>{r.matched}</b> {t('views.import.matched')}
            </span>
            <span className={r.unmatched ? 'miss' : ''}>
              <b>{r.unmatched}</b> {t('views.import.unmatched')}
            </span>
            {r.playlists.length > 0 && (
              <button className="vw-link" onClick={() => setState({ view: 'playlists', playlist: r.playlists.length === 1 ? r.playlists[0] : null })}>
                {t('views.import.playlists', { n: r.playlists.length })}
              </button>
            )}
          </div>
          {r.unmatchedItems.length > 0 && (
            <details className="vw-report-miss">
              <summary>{t('views.import.showMissing')}</summary>
              <ul>
                {r.unmatchedItems.map((p) => (
                  <li key={p} title={p}>
                    {p}
                  </li>
                ))}
              </ul>
              {r.unmatched > r.unmatchedItems.length && <p className="vw-hint">{t('views.import.andMore', { n: r.unmatched - r.unmatchedItems.length })}</p>}
            </details>
          )}
        </>
      )}
    </div>
  )
}

/** Ajustes section. */
export function ImportSection() {
  const t = useT()
  return (
    <section className="set-sec">
      <h3>{t('views.import.title')}</h3>
      <p className="set-hint">{t('views.import.hint')}</p>
      <ImportButtons only={['aimp', 'itunes', 'm3u']} />
      <h4 className="vw-sub">{t('views.import.backup')}</h4>
      <p className="set-hint">{t('views.import.backupHint')}</p>
      <ImportButtons only={['backupOut', 'backupIn']} />
    </section>
  )
}
