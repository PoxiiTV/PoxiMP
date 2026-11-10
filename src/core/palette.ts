import { useEffect, useState } from 'react'

/** Hue in degrees, saturation 0..1 */
export interface Hs {
  h: number
  s: number
}
export interface Palette {
  /** Dominant hue of the darker areas (backgrounds) */
  base: Hs
  /** Most vivid mid-light hue (accents) */
  accent: Hs
}

const BINS = 24

function hsl(r: number, g: number, b: number): [number, number, number] {
  r /= 255
  g /= 255
  b /= 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  const d = max - min
  if (!d) return [0, 0, l]
  const s = d / (1 - Math.abs(2 * l - 1))
  const h = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4
  return [h * 60, s, l]
}

/** Hue histogram → winning hue (circular mean of the best bin and its neighbours) and mean saturation. */
function pick(w: Float32Array, cos: Float32Array, sin: Float32Array, sat: Float32Array): Hs & { weight: number } {
  let best = 0
  let bestW = -1
  for (let i = 0; i < BINS; i++) {
    const sum = w[i] + 0.5 * (w[(i + 1) % BINS] + w[(i + BINS - 1) % BINS])
    if (sum > bestW) [best, bestW] = [i, sum]
  }
  let c = 0
  let s = 0
  let st = 0
  let tw = 0
  for (const i of [best - 1, best, best + 1].map((x) => (x + BINS) % BINS)) {
    c += cos[i]
    s += sin[i]
    st += sat[i]
    tw += w[i]
  }
  return { h: ((Math.atan2(s, c) * 180) / Math.PI + 360) % 360, s: tw ? st / tw : 0, weight: bestW }
}

/** Dominant colours of an image on a 32×32 downscale. null if it has no colour to speak of
 *  (grey cover) or its pixels cannot be read (cross-origin without CORS). */
export function extractPalette(img: HTMLImageElement): Palette | null {
  const N = 32
  const ctx = Object.assign(document.createElement('canvas'), { width: N, height: N }).getContext('2d', { willReadFrequently: true })
  if (!ctx) return null
  ctx.drawImage(img, 0, 0, N, N)
  let data: Uint8ClampedArray
  try {
    data = ctx.getImageData(0, 0, N, N).data
  } catch {
    return null
  }
  const mk = (): Float32Array[] => [0, 0, 0, 0].map(() => new Float32Array(BINS))
  const [aw, ac, as, asat] = mk()
  const [bw, bc, bs, bsat] = mk()
  for (let i = 0; i < data.length; i += 4) {
    if (data[i + 3] < 128) continue
    const [h, s, l] = hsl(data[i], data[i + 1], data[i + 2])
    if (s < 0.08) continue
    const bin = Math.floor((h / 360) * BINS) % BINS
    const rad = (h * Math.PI) / 180
    // Accents are drawn mid-light, so only mid-light pixels vote: a big dark sky must not beat a bright sun
    const vivid = s * s * Math.max(0, 1 - Math.abs(l - 0.58) * 3)
    const dark = s * (1 - l)
    aw[bin] += vivid
    ac[bin] += Math.cos(rad) * vivid
    as[bin] += Math.sin(rad) * vivid
    asat[bin] += s * vivid
    bw[bin] += dark
    bc[bin] += Math.cos(rad) * dark
    bs[bin] += Math.sin(rad) * dark
    bsat[bin] += s * dark
  }
  const accent = pick(aw, ac, as, asat)
  if (accent.weight < 4) return null
  const base = pick(bw, bc, bs, bsat)
  return { accent: { h: accent.h, s: accent.s }, base: { h: base.h, s: base.s } }
}

const cache = new Map<string, Palette | null>()

/** Palette of the image at `url`: undefined while loading, null on error or when it has no colour. */
export function usePalette(url: string | null): Palette | null | undefined {
  const [, bump] = useState(0)
  useEffect(() => {
    if (!url || cache.has(url)) return
    const img = new Image()
    img.crossOrigin = 'anonymous'
    img.onload = () => {
      cache.set(url, extractPalette(img))
      bump((n) => n + 1)
    }
    img.onerror = () => {
      cache.set(url, null)
      bump((n) => n + 1)
    }
    img.src = url
  }, [url])
  return url ? cache.get(url) : null
}
