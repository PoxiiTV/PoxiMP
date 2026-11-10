// Sound settings for Ajustes: output device + bit-perfect mode (Salida de audio), EQ (10 bands,
// preamp, bypass, presets) and AutoEq (Ecualizador), crossfade and volume normalization
// (Reproducción), and the global shortcuts list (Atajos de teclado).
// Structure lives in settings-base.css; the shell styles each section as a card.
import { useEffect, useId, useRef, useState, type FormEvent, type SyntheticEvent } from 'react'
import { api } from '../core/api'
import { num } from '../core/format'
import { errorText, useT } from '../core/i18n'
import { useStore } from '../core/store'
import type { AutoEqEntry, EqBand, EqSettings, EqState, NormMode, OutputDevices, OutputSettings, ShortcutInfo } from '../core/types'
import { fxActive } from './Effects'
import * as Ico from './shell/icons'
import { isMac } from './WindowControls'

const MAX_DB = 12
const NORM_MODES: NormMode[] = ['off', 'track', 'album', 'boostQuiet']
const hasGain = (b: EqBand): boolean => b.kind !== 'lowPass' && b.kind !== 'highPass'

/** 1000 → "1k", 3684 → "3,7k", 62 → "62" */
const freqLabel = (f: number): string => (f >= 1000 ? `${num(f / 1000, f >= 10000 ? 0 : 1)}k` : String(Math.round(f)))
/** +3,5 / −2 / 0 */
const dbLabel = (v: number): string => (Math.abs(v) < 0.05 ? '0' : `${v > 0 ? '+' : '−'}${num(Math.abs(v), 1)}`)

/** Sends curve edits without flooding IPC: one in flight, only the newest one queued. */
function useEqSender(onState: (s: EqState) => void) {
  const busy = useRef(false)
  const queued = useRef<[EqSettings, string | null] | null>(null)
  const send = (eq: EqSettings, preset: string | null): void => {
    if (busy.current) {
      queued.current = [eq, preset]
      return
    }
    busy.current = true
    api
      .eqSet(eq, preset)
      // A newer edit is waiting: this answer is already stale, the next one wins.
      .then((s) => !queued.current && onState(s), () => {})
      .finally(() => {
        busy.current = false
        const q = queued.current
        queued.current = null
        if (q) send(...q)
      })
  }
  return send
}

/** EQ and output state of one Ajustes page, loaded when it opens. */
function useSound() {
  const [st, setSt] = useState<EqState | null>(null)
  const [out, setOut] = useState<OutputDevices | null>(null)
  useEffect(() => {
    api.eqGet().then(setSt, () => {})
    api.outputDevices().then(setOut, () => {})
  }, [])
  const send = useEqSender(setSt)
  /** Local first (sliders follow the pointer), then the engine. */
  const change = (eq: EqSettings, preset: string | null = null): void => {
    if (st) setSt({ ...st, eq, preset })
    send(eq, preset)
  }
  /** Same for the output settings; on a refusal, back to what the backend has. */
  const saveOut = (settings: OutputSettings): void => {
    setOut((o) => o && { ...o, settings })
    api.outputSet(settings).then(
      (s) => setOut((o) => o && { ...o, settings: s }),
      () => void api.outputDevices().then(setOut, () => {})
    )
  }
  /** Devices come and go (a DAC plugged in): re-read the list when the picker gets focus. */
  const refreshDevices = (): void => void api.outputDevices().then((o) => setOut((cur) => cur && { ...cur, devices: o.devices }), () => {})
  return { st, setSt, change, out, saveOut, refreshDevices }
}

/** Ajustes → Salida de audio */
export function OutputPage() {
  const { st, out, saveOut, refreshDevices } = useSound()
  const [fxOn, setFxOn] = useState(false)
  useEffect(() => void api.fxGet().then((f) => setFxOn(fxActive(f)), () => {}), [])
  if (!st || !out) return null
  return <OutputSection out={out} save={saveOut} refresh={refreshDevices} dspOn={st.eq.enabled || st.normalization !== 'off' || fxOn} />
}

/** Ajustes → Ecualizador */
export function EqPage() {
  const { st, setSt, change, out } = useSound()
  if (!st) return null
  return (
    <>
      <EqSection st={st} setSt={setSt} change={change} bitPerfect={!!out?.settings.bitPerfect} />
      <AutoEqSection st={st} setSt={setSt} />
    </>
  )
}

/** Ajustes → Reproducción */
export function PlaybackPage() {
  const { st, setSt, out, saveOut } = useSound()
  if (!st) return null
  return (
    <>
      {out && <CrossfadeSection settings={out.settings} save={saveOut} />}
      <NormSection st={st} setSt={setSt} />
    </>
  )
}

function OutputSection({ out, save, refresh, dspOn }: { out: OutputDevices; save: (s: OutputSettings) => void; refresh: () => void; dspOn: boolean }) {
  const t = useT()
  const info = useStore((s) => s.output)
  const { devices, settings } = out
  const def = devices.find((d) => d.isDefault)
  const missing = settings.deviceId !== null && !devices.some((d) => d.id === settings.deviceId)
  const conflict = settings.bitPerfect && (dspOn || settings.crossfade > 0)
  // A fallback is news only when it concerns what was asked for.
  const fallback = info?.fallback && (settings.bitPerfect || info.fallback === 'deviceMissing') ? info.fallback : null
  return (
    <section className="set-sec out-sec">
      <h3>{t('output.title')}</h3>
      <p className="set-hint">{t('output.hint')}</p>
      <label className="out-device">
        <span className="eq-label">{t('output.device')}</span>
        <select className="eq-input" value={settings.deviceId ?? ''} onFocus={refresh} onChange={(e) => save({ ...settings, deviceId: e.target.value || null })}>
          <option value="">{def ? t('output.systemDefaultNamed', { name: def.name }) : t('output.systemDefault')}</option>
          {devices.map((d) => (
            <option key={d.id} value={d.id}>
              {d.name}
            </option>
          ))}
          {missing && settings.deviceId && <option value={settings.deviceId}>{t('output.missing')}</option>}
        </select>
      </label>
      {info && info.sampleRate > 0 && (
        <p className="out-now">
          {t('output.now')}: <b>{info.device}</b> · {num(info.sampleRate / 1000)} kHz{info.bits ? ` · ${info.bits} bit` : ''} ·{' '}
          {info.exclusive ? t('player.exclusive') : t('output.shared')}
          {info.bitPerfect && <span className="out-badge">{t('player.bitPerfect')}</span>}
        </p>
      )}
      <div className="out-bp">
        <Switch label={t('output.bitPerfect')} checked={settings.bitPerfect} onChange={(bitPerfect) => save({ ...settings, bitPerfect })} />
      </div>
      <p className="set-hint">{t('output.bitPerfectHint')}</p>
      {conflict && <p className="set-hint warn">{t('output.bitPerfectConflict')}</p>}
      <p className="set-hint warn out-fallback" aria-live="polite">
        {fallback ? t(`output.fallback.${fallback}`) : null}
      </p>
    </section>
  )
}

function CrossfadeSection({ settings, save }: { settings: OutputSettings; save: (s: OutputSettings) => void }) {
  const t = useT()
  // The length is kept while the crossfade is off, so switching it back on restores it.
  const [len, setLen] = useState(settings.crossfade || 5)
  const blocked = settings.bitPerfect
  const on = settings.crossfade > 0 && !blocked
  /** Saved when the slider is let go (not on every step of a drag). */
  const commit = (e: SyntheticEvent<HTMLInputElement>): void => {
    const v = Number(e.currentTarget.value)
    if (on && v !== settings.crossfade) save({ ...settings, crossfade: v })
  }
  return (
    <section className="set-sec xfade-sec">
      <div className="eq-head">
        <h3>{t('crossfade.title')}</h3>
        <Switch label={t('crossfade.on')} checked={on} disabled={blocked} onChange={(v) => save({ ...settings, crossfade: v ? len : 0 })} />
      </div>
      <p className="set-hint">{blocked ? t('crossfade.bitPerfect') : t('crossfade.hint')}</p>
      <div className="eq-preamp">
        <span className="eq-label" id="xfade-len-label">
          {t('crossfade.length')}
        </span>
        <input
          type="range"
          className="eq-range"
          min={1}
          max={12}
          step={1}
          value={len}
          disabled={!on}
          aria-labelledby="xfade-len-label"
          aria-valuetext={t('crossfade.secs', { n: len })}
          onChange={(e) => setLen(Number(e.target.value))}
          onPointerUp={commit}
          onKeyUp={commit}
        />
        <output className="eq-db">{t('crossfade.secs', { n: len })}</output>
      </div>
      <div className="xfade-albums">
        <Switch label={t('crossfade.keepAlbums')} checked={settings.keepAlbumsGapless} disabled={!on} onChange={(keepAlbumsGapless) => save({ ...settings, keepAlbumsGapless })} />
      </div>
      <p className="set-hint">{t('crossfade.keepAlbumsHint')}</p>
    </section>
  )
}

interface SectionProps {
  st: EqState
  setSt: (s: EqState) => void
}

function EqSection({ st, setSt, change, bitPerfect }: SectionProps & { change: (eq: EqSettings, preset?: string | null) => void; bitPerfect: boolean }) {
  const t = useT()
  const { eq } = st
  const setBand = (i: number, gain: number): void => change({ ...eq, bands: eq.bands.map((b, j) => (j === i ? { ...b, gain } : b)) })
  const presetName = (id: string | null): string => {
    if (!id) return t('eq.custom')
    if (id.startsWith('autoeq:')) return t('eq.fromAutoEq', { name: id.slice(7) })
    const p = st.presets.find((x) => x.id === id)
    return p ? (p.builtin ? t(`eq.preset.${p.name}`) : p.name) : t('eq.custom')
  }
  const clip = eq.enabled && !eq.autoPreamp ? st.maxBoost + eq.preamp : 0

  return (
    <section className="set-sec eq-sec">
      <div className="eq-head">
        <h3>{t('eq.title')}</h3>
        <Switch label={t('eq.on')} checked={eq.enabled} onChange={(enabled) => change({ ...eq, enabled }, st.preset)} />
      </div>
      <p className="set-hint">{t('eq.hint')}</p>
      {bitPerfect && eq.enabled && <p className="set-hint warn">{t('output.eqBypassed')}</p>}

      <div className="eq-presets" role="radiogroup" aria-label={t('eq.presets')}>
        {st.presets.map((p) => {
          const on = st.preset === p.id
          const name = p.builtin ? t(`eq.preset.${p.name}`) : p.name
          return (
            <span key={p.id} className={`eq-chip ${on ? 'on' : ''} ${p.builtin ? '' : 'user'}`}>
              <button role="radio" aria-checked={on} onClick={() => change({ ...p.eq, enabled: true, autoPreamp: eq.autoPreamp, preamp: eq.preamp }, p.id)}>
                {name}
              </button>
              {!p.builtin && (
                <button className="eq-chip-x" aria-label={t('eq.deletePreset', { name })} title={t('eq.deletePreset', { name })} onClick={() => void api.eqDeletePreset(p.id).then(setSt, () => {})}>
                  <Ico.Close />
                </button>
              )}
            </span>
          )
        })}
      </div>
      <p className="eq-curve">
        {t('eq.curve')}: <b>{presetName(st.preset)}</b>
      </p>

      <div className={`eq-bands ${eq.enabled ? '' : 'off'}`} role="group" aria-label={t('eq.bands')} style={{ gridTemplateColumns: `repeat(${Math.max(eq.bands.length, 1)}, minmax(0, 1fr))` }}>
        {eq.bands.map((b, i) => (
          <BandSlider key={i} band={b} disabled={!eq.enabled || !hasGain(b)} onChange={(g) => setBand(i, g)} />
        ))}
      </div>

      <div className="eq-preamp">
        <span className="eq-label" id="eq-preamp-label">
          {t('eq.preamp')}
        </span>
        <input
          type="range"
          className="eq-range"
          min={-MAX_DB}
          max={MAX_DB}
          step={0.5}
          value={eq.autoPreamp ? st.preamp : eq.preamp}
          disabled={eq.autoPreamp || !eq.enabled}
          aria-labelledby="eq-preamp-label"
          aria-valuetext={t('eq.db', { v: dbLabel(eq.autoPreamp ? st.preamp : eq.preamp) })}
          onChange={(e) => change({ ...eq, preamp: Number(e.target.value) }, st.preset)}
        />
        <output className="eq-db">{t('eq.db', { v: dbLabel(eq.autoPreamp ? st.preamp : eq.preamp) })}</output>
        <Switch label={t('eq.autoPreamp')} checked={eq.autoPreamp} onChange={(autoPreamp) => change({ ...eq, autoPreamp, preamp: autoPreamp ? eq.preamp : Math.round(st.preamp * 2) / 2 }, st.preset)} />
      </div>
      <p className={`set-hint eq-preamp-hint ${clip > 0.05 ? 'warn' : ''}`} aria-live="polite">
        {clip > 0.05
          ? t('eq.clipWarn', { db: num(clip, 1) })
          : eq.autoPreamp
            ? st.maxBoost > 0.05
              ? t('eq.autoPreampOn', { db: num(st.maxBoost, 1) })
              : t('eq.autoPreampFlat')
            : null}
      </p>

      <SavePreset setSt={setSt} />
    </section>
  )
}

function BandSlider({ band, disabled, onChange }: { band: EqBand; disabled: boolean; onChange: (gain: number) => void }) {
  const t = useT()
  const id = useId()
  const freq = `${freqLabel(band.freq)} Hz`
  const kind = t(`eq.kindShort.${band.kind}`)
  const value = hasGain(band) ? band.gain : 0
  return (
    <div className={`eq-band ${disabled ? 'off' : ''}`} title={`${t(`eq.kind.${band.kind}`)} · ${freq} · Q ${num(band.q, 2)}${hasGain(band) ? ` · ${t('eq.resetBand')}` : ''}`}>
      <output className="eq-db" htmlFor={id}>
        {hasGain(band) ? dbLabel(value) : '—'}
      </output>
      <input
        id={id}
        type="range"
        className="eq-range eq-vertical"
        min={-MAX_DB}
        max={MAX_DB}
        step={0.5}
        value={Math.max(-MAX_DB, Math.min(MAX_DB, value))}
        disabled={disabled}
        aria-label={t('eq.band', { freq })}
        aria-valuetext={t('eq.db', { v: dbLabel(value) })}
        onChange={(e) => onChange(Number(e.target.value))}
        onDoubleClick={() => !disabled && onChange(0)}
      />
      <span className="eq-freq">{freqLabel(band.freq)}</span>
      {kind && <span className="eq-kind">{kind}</span>}
    </div>
  )
}

export function Switch({ label, checked, disabled, onChange }: { label: string; checked: boolean; disabled?: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="eq-switch">
      <input type="checkbox" role="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <i aria-hidden="true" />
      <span>{label}</span>
    </label>
  )
}

function SavePreset({ setSt }: { setSt: (s: EqState) => void }) {
  const t = useT()
  const [open, setOpen] = useState(false)
  const [name, setName] = useState('')
  const [err, setErr] = useState('')
  const submit = (e: FormEvent): void => {
    e.preventDefault()
    if (!name.trim()) return
    api.eqSavePreset(name).then(
      (s) => {
        setSt(s)
        setOpen(false)
        setName('')
        setErr('')
      },
      (x) => setErr(errorText(x))
    )
  }
  if (!open)
    return (
      <div className="set-actions">
        <button className="btn" onClick={() => setOpen(true)}>
          {t('eq.savePreset')}
        </button>
      </div>
    )
  return (
    <form className="set-actions eq-save" onSubmit={submit}>
      <input className="eq-input" autoFocus maxLength={40} value={name} placeholder={t('eq.presetName')} aria-label={t('eq.presetName')} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Escape' && setOpen(false)} />
      <button className="btn primary" type="submit" disabled={!name.trim()}>
        {t('eq.save')}
      </button>
      <button className="btn" type="button" onClick={() => setOpen(false)}>
        {t('eq.cancel')}
      </button>
      {err && <p className="set-hint warn">{err}</p>}
    </form>
  )
}

function AutoEqSection({ st, setSt }: SectionProps) {
  const t = useT()
  const [q, setQ] = useState('')
  const [hits, setHits] = useState<AutoEqEntry[] | null>(null)
  const [busy, setBusy] = useState(false)
  const [applying, setApplying] = useState<string | null>(null)
  const [err, setErr] = useState('')
  const gen = useRef(0)
  useEffect(() => {
    const query = q.trim()
    if (!query) {
      setHits(null)
      setBusy(false)
      return
    }
    const my = ++gen.current
    setBusy(true)
    // Debounced: the first search may download the index (~1 MB), later ones are instant.
    const timer = setTimeout(() => {
      api.autoEqSearch(query).then(
        (r) => {
          if (my !== gen.current) return
          setHits(r)
          setErr('')
          setBusy(false)
        },
        (x) => {
          if (my !== gen.current) return
          setHits(null)
          setErr(errorText(x))
          setBusy(false)
        }
      )
    }, 220)
    return () => clearTimeout(timer)
  }, [q])
  const applied = st.preset?.startsWith('autoeq:') ? st.preset.slice(7) : null
  const apply = (e: AutoEqEntry): void => {
    setApplying(e.path)
    setErr('')
    api
      .autoEqApply(e.path)
      .then(setSt, (x) => setErr(errorText(x)))
      .finally(() => setApplying(null))
  }

  return (
    <section className="set-sec autoeq-sec">
      <h3>{t('autoeq.title')}</h3>
      <p className="set-hint">{t('autoeq.hint')}</p>
      <input className="eq-input autoeq-search" type="search" value={q} placeholder={t('autoeq.placeholder')} aria-label={t('autoeq.search')} onChange={(e) => setQ(e.target.value)} />
      <p className="set-hint" aria-live="polite">
        {busy ? t('autoeq.searching') : hits && !hits.length ? t('autoeq.none', { q: q.trim() }) : ''}
      </p>
      {err && <p className="set-hint warn">{err}</p>}
      {hits && hits.length > 0 && (
        <ul className="autoeq-list">
          {hits.map((e) => {
            const on = applied === e.name
            return (
              <li key={e.path} className={on ? 'on' : ''}>
                <span className="autoeq-name">{e.name}</span>
                <span className="autoeq-meta">
                  {t(`autoeq.form.${e.form}`)} · {t('autoeq.by', { source: e.source })}
                </span>
                <button className={`btn ${on ? '' : 'primary'}`} disabled={!!applying} aria-label={t('autoeq.applyTo', { name: e.name })} onClick={() => apply(e)}>
                  {applying === e.path ? t('autoeq.applying') : on ? t('autoeq.applied') : t('autoeq.apply')}
                </button>
              </li>
            )
          })}
        </ul>
      )}
    </section>
  )
}

function NormSection({ st, setSt }: SectionProps) {
  const t = useT()
  const set = (mode: NormMode): void => {
    setSt({ ...st, normalization: mode })
    api.setNormalization(mode).then(setSt, () => {})
  }
  return (
    <section className="set-sec norm-sec">
      <h3>{t('norm.title')}</h3>
      <p className="set-hint">{t('norm.hint')}</p>
      <div className="seg norm-modes" role="radiogroup" aria-label={t('norm.title')}>
        {NORM_MODES.map((m) => (
          <button key={m} role="radio" aria-checked={st.normalization === m} className={st.normalization === m ? 'on' : ''} onClick={() => set(m)}>
            {t(`norm.mode.${m}`)}
          </button>
        ))}
      </div>
      <p className="set-hint">{t(`norm.modeHint.${st.normalization}`)}</p>
    </section>
  )
}

export function ShortcutsSection() {
  const t = useT()
  const [list, setList] = useState<ShortcutInfo[]>([])
  useEffect(() => {
    api.shortcuts().then(setList, () => {})
  }, [])
  if (!list.length) return null
  const key = (k: string): string => t(`shortcuts.key.${isMac && k === 'alt' ? 'option' : k}`)
  return (
    <section className="set-sec keys-sec">
      <h3>{t('shortcuts.title')}</h3>
      <p className="set-hint">{t('shortcuts.hint')}</p>
      <dl className="keys-list">
        {list.map((s) => (
          <div key={s.id} className={s.active ? '' : 'off'}>
            <dt>{t(`shortcuts.${s.id}`)}</dt>
            <dd>
              {s.keys.map((k) => (
                <kbd key={k}>{key(k)}</kbd>
              ))}
              {!s.active && <span className="keys-taken">{t('shortcuts.taken')}</span>}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  )
}
