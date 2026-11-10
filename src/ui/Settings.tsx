// Ajustes sections (the screen itself is ui/shell/SettingsScreen.tsx). Each section is a few
// titled cards (.set-sec); the sound ones live in Equalizer.tsx.
import { useEffect, useId, useState, type ReactNode } from 'react'
import { api, native } from '../core/api'
import { addFolder, removeFolder, setQualitySettings } from '../core/hooks'
import { useT, type Lang } from '../core/i18n'
import { setPrefs, setSkin, useStore, ZOOMS } from '../core/store'
import type { RemoteInfo } from '../core/types'
import { toggleMini } from '../core/window'
import { skins } from '../skins/registry'
import { ShortcutsSection, Switch } from './Equalizer'
import { ImportSection } from './views/Import'
import { offerUpdate } from './Updater'
import * as Ico from './shell/icons'
import { isMac } from './WindowControls'
import './settings-base.css'

/** One row of a card: title and hint on the left, the control on the right. */
export function Opt({ title, hint, children }: { title: string; hint?: ReactNode; children?: ReactNode }) {
  const id = useId()
  return (
    <div className="opt">
      <div className="tx" id={id}>
        <b>{title}</b>
        {hint && <span>{hint}</span>}
      </div>
      {children && <div className="ctl" aria-describedby={id}>{children}</div>}
    </div>
  )
}

/** A bare switch for an Opt row (its title is the label). */
export function Toggle({ label, checked, disabled, onChange }: { label: string; checked: boolean; disabled?: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="eq-switch">
      <input type="checkbox" role="switch" aria-label={label} checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <i aria-hidden="true" />
    </label>
  )
}

export function Seg<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div className="seg" role="radiogroup" aria-label={label}>
      {options.map(([v, text]) => (
        <button key={v} role="radio" aria-checked={value === v} className={value === v ? 'on' : ''} onClick={() => onChange(v)}>
          {text}
        </button>
      ))}
    </div>
  )
}

// ---------- General ----------

export function GeneralSection() {
  const t = useT()
  const prefs = useStore((s) => s.prefs)
  const key = isMac ? '⌘ M' : 'Ctrl M'
  return (
    <>
      <section className="set-sec">
        <h3>{t('set.general.start')}</h3>
        <Opt title={t('set.general.lang')}>
          <select className="eq-input" value={prefs.lang ?? ''} aria-label={t('set.general.lang')} onChange={(e) => setPrefs({ lang: (e.target.value || null) as Lang | null })}>
            <option value="">{t('set.general.langSystem')}</option>
            <option value="es">Español</option>
            <option value="en">English</option>
          </select>
        </Opt>
      </section>
      <section className="set-sec">
        <h3>{t('set.general.window')}</h3>
        <Opt title={t('settings.mini')} hint={t('settings.miniHint', { key })}>
          <button className="btn" onClick={() => void toggleMini().catch(() => {})}>
            {t('settings.miniOpen')}
          </button>
        </Opt>
        <Opt title={t('set.general.miniOnTop')} hint={t('set.general.miniOnTopHint')}>
          <Toggle label={t('set.general.miniOnTop')} checked={prefs.miniOnTop} onChange={(miniOnTop) => setPrefs({ miniOnTop })} />
        </Opt>
        <Opt title={t('set.general.onTop')} hint={t('set.general.onTopHint')}>
          <Toggle label={t('set.general.onTop')} checked={prefs.onTop} onChange={(onTop) => setPrefs({ onTop })} />
        </Opt>
      </section>
      {/* macOS: Finder's "Open with" → "Change all" does it; there is no settings page to open */}
      {!isMac && (
        <section className="set-sec">
          <h3>{t('set.general.files')}</h3>
          <Opt title={t('set.general.defaultApp')} hint={t('set.general.defaultAppHint')}>
            <button className="btn" onClick={() => void api.openDefaultApps().catch(() => {})}>
              {t('set.general.defaultAppOpen')}
            </button>
          </Opt>
        </section>
      )}
    </>
  )
}

// ---------- Apariencia ----------

export function AppearanceSection() {
  const t = useT()
  const current = useStore((s) => s.skin)
  const font = useStore((s) => s.prefs.font)
  const zoom = useStore((s) => s.prefs.zoom)
  const variants = skins.find((s) => s.id === current.id)?.variants ?? []
  return (
    <>
      <section className="set-sec">
        <h3>{t('settings.skins')}</h3>
        <div className="sk" role="radiogroup" aria-label={t('settings.skins')}>
          {skins.map((s) => {
            const on = s.id === current.id
            return (
              <button key={s.id} role="radio" aria-checked={on} className={on ? 'on' : ''} onClick={() => !on && setSkin({ id: s.id, variant: s.variants[0] })}>
                <i style={{ background: s.preview }} aria-hidden="true" />
                <span>{t(`skins.${s.id}`)}</span>
              </button>
            )
          })}
        </div>
        <Opt title={t('set.look.variant')} hint={variants.length > 1 ? undefined : t('set.look.variantNone')}>
          {variants.length > 1 && (
            <Seg label={t('set.look.variant')} value={current.variant} options={variants.map((v) => [v, t(`settings.variant.${v}`)])} onChange={(variant) => setSkin({ id: current.id, variant })} />
          )}
        </Opt>
      </section>
      <section className="set-sec">
        <h3>{t('set.look.text')}</h3>
        <Opt title={t('set.look.font')} hint={t('set.look.fontHint')}>
          <Seg label={t('set.look.font')} value={font} options={[['skin', t('set.look.fontSkin')], ['neutral', t('set.look.fontNeutral')]]} onChange={(f) => setPrefs({ font: f })} />
        </Opt>
        <Opt title={t('set.look.zoom')} hint={t('set.look.zoomHint')}>
          <Seg label={t('set.look.zoom')} value={String(zoom)} options={ZOOMS.map((z) => [String(z), `${Math.round(z * 100)} %`])} onChange={(z) => setPrefs({ zoom: Number(z) })} />
        </Opt>
      </section>
    </>
  )
}

// ---------- Biblioteca ----------

export function LibrarySection() {
  const t = useT()
  const scan = useStore((s) => s.scan)
  const [folders, setFolders] = useState<string[]>([])
  useEffect(() => {
    api.folders().then(setFolders, () => {})
  }, [])
  return (
    <section className="set-sec">
      <h3>{t('settings.folders')}</h3>
      <p className="set-hint">{t('settings.foldersHint')}</p>
      {folders.length ? (
        <ul className="folder-list">
          {folders.map((f) => (
            <li key={f} title={f} className="folder-row">
              <span>{f}</span>
              <button
                className="folder-x"
                aria-label={t('settings.removeFolder', { path: f })}
                title={t('settings.removeFolderHint')}
                onClick={() => void removeFolder(f).then(setFolders, () => {})}
              >
                <Ico.Close />
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="set-empty">{t('settings.noFolders')}</p>
      )}
      <div className="set-actions">
        <button className="btn primary" onClick={() => void addFolder().then((f) => f && setFolders(f), () => {})}>
          {t('settings.addFolder')}
        </button>
        {folders.length > 0 && (
          <button className="btn" disabled={!!scan} onClick={() => void api.scan().catch(() => {})}>
            {t('settings.rescan')}
          </button>
        )}
      </div>
      {scan && <p className="set-hint">{t('library.scanning', { done: scan.done, total: scan.total })}</p>}
    </section>
  )
}

// ---------- Calidad real ----------

export function QualitySection() {
  const t = useT()
  const q = useStore((s) => s.qset)
  return (
    <section className="set-sec">
      <h3>{t('set.quality.detector')}</h3>
      <Opt title={t('set.quality.analyze')} hint={t('set.quality.analyzeHint')}>
        <Toggle label={t('set.quality.analyze')} checked={q.analyze} onChange={(analyze) => setQualitySettings({ ...q, analyze })} />
      </Opt>
      <Opt title={t('set.quality.noSigns')} hint={t('set.quality.noSignsHint')}>
        <Toggle label={t('set.quality.noSigns')} checked={q.showNoSigns} disabled={!q.analyze} onChange={(showNoSigns) => setQualitySettings({ ...q, showNoSigns })} />
      </Opt>
    </section>
  )
}

// ---------- Letras ----------

/** The online-lyrics switch (LRCLIB). Local lyrics (tags, .lrc) are always read. */
export function LyricsSection() {
  const t = useT()
  const [on, setOn] = useState<boolean | null>(null)
  useEffect(() => {
    api.lyricsOnline().then(setOn, () => {})
  }, [])
  const set = (v: boolean): void => {
    setOn(v)
    api.setLyricsOnline(v).then(setOn, () => setOn(!v))
  }
  return (
    <section className="set-sec">
      <h3>{t('settings.lyrics')}</h3>
      <Opt title={t('settings.lyricsOnline')} hint={t('settings.lyricsHint')}>
        <Toggle label={t('settings.lyricsOnline')} checked={!!on} disabled={on === null} onChange={set} />
      </Opt>
    </section>
  )
}

// ---------- Mando desde el móvil ----------

/** Phone remote: a small server on the home network (off by default) and the QR that pairs a phone. */
export function RemoteSection() {
  const t = useT()
  const [info, setInfo] = useState<RemoteInfo | null>(null)
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  const on = !!info?.on
  useEffect(() => {
    api.remoteGet().then(setInfo, () => {})
  }, [])
  // Phones come and go: refreshed while it is on.
  useEffect(() => {
    if (!on) return
    const id = setInterval(() => void api.remoteGet().then(setInfo, () => {}), 3000)
    return () => clearInterval(id)
  }, [on])
  const run = (p: Promise<RemoteInfo>): void => {
    setBusy(true)
    setFailed(false)
    p.then(setInfo, () => setFailed(true)).finally(() => setBusy(false))
  }
  const noNetwork = info !== null && info.ips.length === 0
  return (
    <section className="set-sec remote-sec">
      <div className="eq-head">
        <h3>{t('settings.remote.title')}</h3>
        <Switch label={t('settings.remote.on')} checked={on} disabled={!info || busy || (noNetwork && !on)} onChange={(v) => run(api.remoteSet(v))} />
      </div>
      <p className="set-hint">{t('settings.remote.hint')}</p>
      {noNetwork && <p className="set-hint warn">{t('settings.remote.noNetwork')}</p>}
      {failed && (
        <p className="set-hint warn" role="alert">
          {t('settings.remote.failed')}
        </p>
      )}
      {info?.on && info.url && info.qr && (
        <>
          {info.ips.length > 1 && (
            <div className="out-device">
              <label className="eq-label" htmlFor="remote-ip">
                {t('settings.remote.network')}
              </label>
              <select id="remote-ip" className="eq-input" value={info.ip ?? ''} disabled={busy} onChange={(e) => run(api.remoteSet(true, e.target.value))}>
                {info.ips.map((i) => (
                  <option key={i.ip} value={i.ip}>{`${i.name} · ${i.ip}`}</option>
                ))}
              </select>
            </div>
          )}
          <div className="remote-pair">
            <img className="remote-qr" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(info.qr)}`} alt={t('settings.remote.qrAlt')} width={168} height={168} />
            <div className="remote-text">
              <p>{t('settings.remote.scan')}</p>
              <p className="set-hint">{t('settings.remote.url')}</p>
              <code className="remote-url">{info.url}</code>
              <div className="set-actions">
                <button className="btn" disabled={busy} onClick={() => run(api.remoteRotate())}>
                  {t('settings.remote.newCode')}
                </button>
              </div>
              <p className="set-hint">{t('settings.remote.newCodeHint')}</p>
            </div>
          </div>
          <h4 className="remote-h">{t('settings.remote.phones', { n: info.phones.length })}</h4>
          {info.phones.length ? (
            <ul className="folder-list">
              {info.phones.map((p, i) => (
                <li key={`${p.ip}-${i}`}>
                  {t(`settings.remote.device.${p.device}`)} · {p.ip}
                  <span className="remote-state">{p.live ? t('settings.remote.live') : t('settings.remote.idle')}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="set-empty">{t('settings.remote.noPhones')}</p>
          )}
          <p className="set-hint">{t('settings.remote.firewall')}</p>
        </>
      )}
    </section>
  )
}

// ---------- Atajos ----------

/** In-app keys (fixed) and the global ones (from the backend). */
export function KeysSection() {
  const t = useT()
  const mod = isMac ? '⌘' : 'Ctrl'
  const keys: [string, string[]][] = [
    ['playPause', [t('shortcuts.key.space')]],
    ['search', [mod, 'K']],
    ['mini', [mod, 'M']],
    ['back', ['Esc']]
  ]
  return (
    <>
      <section className="set-sec keys-sec">
        <h3>{t('set.keys.app')}</h3>
        <dl className="keys-list">
          {keys.map(([id, k]) => (
            <div key={id}>
              <dt>{t(`set.keys.${id}`)}</dt>
              <dd>
                {k.map((x) => (
                  <kbd key={x}>{x}</kbd>
                ))}
              </dd>
            </div>
          ))}
        </dl>
      </section>
      <ShortcutsSection />
    </>
  )
}

// ---------- Importar, Acerca de ----------

export { ImportSection }

export function AboutSection() {
  const t = useT()
  const [version, setVersion] = useState('')
  const [checking, setChecking] = useState(false)
  const [result, setResult] = useState<'upToDate' | 'checkError' | null>(null)
  const checkNow = (): void => {
    setChecking(true)
    setResult(null)
    void offerUpdate(true)
      .then((r) => setResult(r === 'none' ? 'upToDate' : r === 'error' ? 'checkError' : null))
      .finally(() => setChecking(false))
  }
  useEffect(() => {
    api.version().then(setVersion, () => {})
  }, [])
  return (
    <section className="set-sec">
      <h3>PoxiMP {version}</h3>
      <p className="set-hint">{t('set.about.text')}</p>
      <Opt title={t('update.check')} hint={t('update.checkHint')}>
        <button className="btn" disabled={checking || !native} onClick={checkNow}>
          {checking ? t('update.checking') : t('update.check')}
        </button>
      </Opt>
      {result && <p className="set-hint" role="status">{t(`update.${result}`)}</p>}
    </section>
  )
}
