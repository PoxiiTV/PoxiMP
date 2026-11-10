// Ajustes → Efectos: presets, headphone crossfeed, stereo width / mono / balance, bass boost and
// speed (pitch follows, like a turntable). Everything but speed glides in the engine; speed
// reopens the decks (a short gap), so it is only sent when the slider is let go.
import { useEffect, useRef, useState, type ReactNode, type SyntheticEvent } from 'react'
import { api } from '../core/api'
import { num } from '../core/format'
import { useT } from '../core/i18n'
import type { Crossfeed, FxSettings } from '../core/types'
import { Opt, Seg, Toggle } from './Settings'

export const FX_NEUTRAL: FxSettings = { crossfeed: 'off', width: 1, mono: false, balance: 0, bass: 0, speed: 1 }
/** Starting points, from neutral */
const PRESETS: [string, FxSettings][] = [
  ['flat', FX_NEUTRAL],
  ['headphones', { ...FX_NEUTRAL, crossfeed: 'medium' }],
  ['bass', { ...FX_NEUTRAL, bass: 6 }],
  ['wide', { ...FX_NEUTRAL, width: 1.4 }]
]
const same = (a: FxSettings, b: FxSettings): boolean => (Object.keys(a) as (keyof FxSettings)[]).every((k) => a[k] === b[k])
const CROSSFEED: Crossfeed[] = ['off', 'soft', 'medium', 'strong']
const pct = (v: number): string => `${Math.round(v * 100)} %`

/** Effects not at their neutral value (a bit-perfect conflict, a dot on the section). */
export const fxActive = (f: FxSettings): boolean =>
  f.crossfeed !== 'off' || f.mono || f.width !== 1 || f.balance !== 0 || f.bass !== 0 || f.speed !== 1

function Range(p: { label: string; min: number; max: number; step: number; value: number; text: string; disabled?: boolean; onInput: (v: number) => void; onCommit?: (v: number) => void }) {
  const commit = (e: SyntheticEvent<HTMLInputElement>): void => p.onCommit?.(Number(e.currentTarget.value))
  return (
    <span className="fx-range">
      <input
        type="range"
        className="eq-range"
        min={p.min}
        max={p.max}
        step={p.step}
        value={p.value}
        disabled={p.disabled}
        aria-label={p.label}
        aria-valuetext={p.text}
        onChange={(e) => p.onInput(Number(e.target.value))}
        onPointerUp={commit}
        onKeyUp={commit}
      />
      <output className="eq-db">{p.text}</output>
    </span>
  )
}

export function EffectsPage() {
  const t = useT()
  const [fx, setFx] = useState<FxSettings | null>(null)
  const [bitPerfect, setBitPerfect] = useState(false)
  useEffect(() => {
    api.fxGet().then(setFx, () => {})
    api.outputDevices().then((o) => setBitPerfect(o.settings.bitPerfect), () => {})
  }, [])
  // One request in flight, only the newest one queued (sliders send while dragging)
  const busy = useRef(false)
  const queued = useRef<FxSettings | null>(null)
  const send = (f: FxSettings): void => {
    if (busy.current) {
      queued.current = f
      return
    }
    busy.current = true
    api
      .fxSet(f)
      .catch(() => {})
      .finally(() => {
        busy.current = false
        const q = queued.current
        queued.current = null
        if (q) send(q)
      })
  }
  if (!fx) return null
  /** Local first, then the engine (`live` = also while dragging) */
  const change = (patch: Partial<FxSettings>, live = true): void => {
    const next = { ...fx, ...patch }
    setFx(next)
    if (live) send(next)
  }
  // Speed is only sent on release: what the engine plays now
  const sendSpeed = (speed: number): void => send({ ...fx, speed })
  const preset = PRESETS.find(([, p]) => same(p, fx))?.[0] ?? ''
  const row = (title: string, hint: string, ctl: ReactNode) => (
    <Opt title={title} hint={hint}>
      {ctl}
    </Opt>
  )
  return (
    <>
      <section className="set-sec">
        <h3>{t('fx.preset')}</h3>
        {row(
          bitPerfect ? t('fx.bitPerfectTitle') : t('fx.presetHint'),
          bitPerfect ? t('fx.bitPerfect') : t('fx.hint'),
          <Seg label={t('fx.preset')} value={preset} options={PRESETS.map(([id]) => [id, t(`fx.presets.${id}`)])} onChange={(id) => change(PRESETS.find(([p]) => p === id)?.[1] ?? FX_NEUTRAL)} />
        )}
      </section>
      <section className="set-sec">
        <h3>{t('fx.headphones')}</h3>
        {row(
          t('fx.crossfeed'),
          t('fx.crossfeedHint'),
          <Seg label={t('fx.crossfeed')} value={fx.crossfeed} options={CROSSFEED.map((c) => [c, t(`fx.cf.${c}`)])} onChange={(crossfeed) => change({ crossfeed })} />
        )}
      </section>
      <section className="set-sec">
        <h3>{t('fx.stereo')}</h3>
        {row(t('fx.width'), t('fx.widthHint'), <Range label={t('fx.width')} min={0} max={2} step={0.05} value={fx.width} text={pct(fx.width)} disabled={fx.mono} onInput={(width) => change({ width })} />)}
        {row(t('fx.mono'), t('fx.monoHint'), <Toggle label={t('fx.mono')} checked={fx.mono} onChange={(mono) => change({ mono })} />)}
        {row(
          t('fx.balance'),
          t('fx.balanceHint'),
          <Range
            label={t('fx.balance')}
            min={-1}
            max={1}
            step={0.05}
            value={fx.balance}
            text={fx.balance === 0 ? t('fx.centre') : `${t(fx.balance < 0 ? 'fx.left' : 'fx.right')} ${Math.round(Math.abs(fx.balance) * 100)}`}
            onInput={(balance) => change({ balance: Math.abs(balance) < 0.025 ? 0 : balance })}
          />
        )}
      </section>
      <section className="set-sec">
        <h3>{t('fx.tone')}</h3>
        {row(t('fx.bass'), t('fx.bassHint'), <Range label={t('fx.bass')} min={0} max={12} step={0.5} value={fx.bass} text={fx.bass ? `+${num(fx.bass, 1)} dB` : '0 dB'} onInput={(bass) => change({ bass })} />)}
        {row(
          t('fx.speed'),
          t('fx.speedHint'),
          <span className="fx-speed">
            <Range label={t('fx.speed')} min={0.5} max={1.5} step={0.05} value={fx.speed} text={`${num(fx.speed, 2)}×`} onInput={(speed) => change({ speed }, false)} onCommit={sendSpeed} />
            <button className="btn" disabled={fx.speed === 1} onClick={() => change({ speed: 1 })}>
              {t('fx.normal')}
            </button>
          </span>
        )}
      </section>
    </>
  )
}
