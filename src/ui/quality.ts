// Quality badge and report logic shared by every skin: verdict → kind + texts. Skins only style it.
import { formatLabel, khz, res } from '../core/format'
import { useQuality, type Quality } from '../core/hooks'
import { t } from '../core/i18n'
import { useStore } from '../core/store'
import type { QualityReport, Track } from '../core/types'

/** ok = Real (lossless, full band) · clean = lossy without signs · warn = doubtful · bad = fake ·
 *  scan = analysing · none = not analysed / no verdict */
export type BadgeKind = 'ok' | 'clean' | 'warn' | 'bad' | 'scan' | 'none'

export interface Badge {
  kind: BadgeKind
  label: string
  /** Secondary text ("· real 128"), may be empty */
  extra: string
  /** Bandwidth bar fill 0..1: how far the sound reaches within what the format promises (or progress) */
  band: number
  /** Only the "Real" badge gets the tick */
  tick: boolean
  /** Where only one short text fits (mini): the real quality ("Real 128 kbps") instead of "Fake" */
  short: string
}

/** Where the sound really ends (Hz) */
const reach = (r: QualityReport): number => r.cutoffHz ?? r.bandwidthHz
const origin = (r: QualityReport): string => t(`quality.origin.${r.origin ?? 'lossy'}`)
const realRes = (r: QualityReport): string => (r.realBits && r.realRate ? res(r.realBits, r.realRate) : '')

/** The badge a track shows, or null when the detector is off (Ajustes → Calidad real) or it is a
 *  plain "no signs" the user chose not to see. */
export function useBadge(track: Track | null): Badge | null {
  const { analyze, showNoSigns } = useStore((s) => s.qset)
  const b = badge(useQuality(analyze ? track : null))
  return !analyze || (b.kind === 'clean' && !showNoSigns) ? null : b
}

export function badge(q: Quality | null): Badge {
  const r = q?.report
  const band = r ? Math.min(1, reach(r) / r.nyquistHz) : 0
  const b = (kind: BadgeKind, label: string, extra = '', fill = band, short = label): Badge => ({ kind, label, extra, band: fill, tick: false, short })
  switch (q?.state) {
    case 'real':
      return { ...b('ok', t('quality.real'), '', 1), tick: true }
    case 'noSigns':
      return b('clean', t('quality.noSigns'))
    case 'fake':
      return r?.estKbps ? b('bad', t('quality.fake'), t('quality.realKbps', { kbps: r.estKbps }), band, t('quality.shortKbps', { kbps: r.estKbps })) : b('bad', t('quality.fake'))
    case 'lossyOrigin':
      return b('bad', t('quality.lossyOrigin', { origin: r ? origin(r) : t('quality.origin.lossy') }), r ? t('quality.cut', { khz: khz(reach(r)) }) : '')
    case 'fakeHiRes':
      return r && realRes(r) ? b('bad', t('quality.fakeHiRes'), t('quality.realRes', { res: realRes(r) }), band, t('quality.shortRes', { res: realRes(r) })) : b('bad', t('quality.fakeHiRes'))
    case 'suspicious':
      return r?.estKbps ? b('warn', t('quality.suspicious'), t('quality.approxKbps', { kbps: r.estKbps }), band, t('quality.shortApprox', { kbps: r.estKbps })) : b('warn', t('quality.suspicious'))
    case 'analyzing':
      return b('scan', t('quality.analyzing'), q.progress !== undefined ? t('quality.pct', { n: Math.round(q.progress * 100) }) : '', q.progress ?? 0)
    case 'inconclusive':
      return b('none', t('quality.inconclusive'), '', 0)
    default:
      return b('none', t('quality.pending'), '', 0)
  }
}

export interface Fact {
  label: string
  value: string
  tone?: 'ok' | 'bad'
}

export interface Detail {
  /** Sentence with <b>…</b> marks, render with rich() */
  text: string
  facts: Fact[]
  /** For the spectrum sketch, kHz; cut === max means full band. null = no data */
  sketch: { max: number; cut: number } | null
}

/** The report card for one track: sentence, four facts, and the spectrum sketch. */
export function detail(track: Track, q: Quality | null): Detail {
  const r = q?.report ?? null
  const state = q?.state ?? 'pending'
  const fmt = formatLabel(track)
  const lossyKbps = `${Math.round(track.bitrate)}\u00a0kbps`
  const dash = '—'
  const conf: Fact = { label: t('quality.facts.confidence'), value: r ? `${Math.round(r.confidence * 100)} %` : dash }
  const sketch = r ? { max: r.nyquistHz / 1000, cut: reach(r) / 1000 } : null
  if (!r || state === 'analyzing' || state === 'pending' || state === 'inconclusive') {
    return {
      text: t(`quality.detail.${state}`),
      facts: [
        { label: t('quality.facts.declared'), value: fmt },
        { label: t('quality.facts.real'), value: dash },
        { label: t('quality.facts.cut'), value: dash },
        conf
      ],
      sketch: state === 'inconclusive' ? sketch : null
    }
  }
  const k = khz(reach(r))
  const declared = track.lossless ? fmt : lossyKbps
  const cut: Fact = { label: t('quality.facts.cut'), value: k }
  switch (state) {
    case 'real':
      return {
        text: t('quality.detail.real', { khz: k, fmt }),
        facts: [{ label: t('quality.facts.declared'), value: fmt }, { label: t('quality.facts.real'), value: track.bitDepth ? res(track.bitDepth, track.sampleRate) : fmt, tone: 'ok' }, { label: t('quality.facts.reach'), value: k }, conf],
        sketch: { max: r.nyquistHz / 1000, cut: r.nyquistHz / 1000 }
      }
    case 'noSigns':
      return {
        text: t('quality.detail.noSigns', { khz: k, fmt }),
        facts: [{ label: t('quality.facts.declared'), value: declared }, { label: t('quality.facts.real'), value: declared, tone: 'ok' }, { label: t('quality.facts.reach'), value: k }, conf],
        sketch
      }
    case 'fake':
      return {
        text: t('quality.detail.fake', { declared: `${track.format} ${lossyKbps}`, khz: k, kbps: r.estKbps ?? '?' }),
        facts: [{ label: t('quality.facts.declared'), value: declared }, { label: t('quality.facts.real'), value: r.estKbps ? `≈${r.estKbps}\u00a0kbps` : dash, tone: 'bad' }, cut, conf],
        sketch
      }
    case 'lossyOrigin':
      return {
        text: t('quality.detail.lossyOrigin', { fmt: track.format, khz: k, origin: origin(r) }),
        facts: [
          { label: t('quality.facts.declared'), value: `${track.format} ${lossyKbps}` },
          { label: t('quality.facts.origin'), value: `${origin(r)}${r.estKbps ? ` ≈${r.estKbps}` : ''}`, tone: 'bad' },
          cut,
          conf
        ],
        sketch
      }
    case 'fakeHiRes': {
      const claimed = track.bitDepth ? res(track.bitDepth, track.sampleRate) : fmt
      return {
        text: t('quality.detail.fakeHiRes', { res: claimed, khz: k, real: realRes(r) || dash }),
        facts: [{ label: t('quality.facts.declared'), value: claimed }, { label: t('quality.facts.real'), value: realRes(r) || dash, tone: 'bad' }, cut, conf],
        sketch
      }
    }
    default: // suspicious
      return {
        text: t('quality.detail.suspicious', { khz: k, fmt }),
        facts: [{ label: t('quality.facts.declared'), value: declared }, { label: t('quality.facts.real'), value: r.estKbps ? `≈${r.estKbps}\u00a0kbps` : dash, tone: 'bad' }, cut, conf],
        sketch
      }
  }
}

/** Splits "a <b>b</b> c" into parts so skins can render the bold bits without HTML. */
export function rich(text: string): { bold: boolean; text: string }[] {
  return text.split(/<b>(.*?)<\/b>/).map((s, i) => ({ bold: i % 2 === 1, text: s }))
}

/** Percent for CSS, e.g. band fill */
export const pct = (f: number): string => `${(Math.max(0, Math.min(1, f)) * 100).toFixed(2)}%`
