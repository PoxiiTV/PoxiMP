import { currentLang, t } from './i18n'
import type { Track } from './types'

const locale = (): string => (currentLang() === 'es' ? 'es-ES' : 'en-GB')

/** Up to `max` decimals, locale separator: 44.1 → "44,1" in Spanish */
export function num(n: number, max = 1): string {
  return n.toLocaleString(locale(), { maximumFractionDigits: max, useGrouping: false })
}

/** 222 → "3:42"; 3725 → "1:02:05" */
export function time(secs: number): string {
  const s = Math.max(0, Math.floor(secs))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const ss = String(s % 60).padStart(2, '0')
  return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`
}

/** Total length: "33 min", "5 h 12 min" */
export function total(secs: number): string {
  const m = Math.round(secs / 60)
  return m < 60 ? t('fmt.min', { n: m }) : t('fmt.hmin', { h: Math.floor(m / 60), m: m % 60 })
}

/** Non-breaking space: "19 kHz" never splits across lines */
export const khz = (hz: number): string => `${num(hz / 1000)}\u00a0kHz`

/** 16, 44100 → "16/44,1" */
export const res = (bits: number, rate: number): string => `${bits}/${num(rate / 1000)}`

/** What the file says it is: "FLAC 16/44,1", "MP3 320" */
export function formatLabel(tr: Track): string {
  return tr.lossless && tr.bitDepth ? `${tr.format} ${res(tr.bitDepth, tr.sampleRate)}` : `${tr.format} ${Math.round(tr.bitrate)}`
}
