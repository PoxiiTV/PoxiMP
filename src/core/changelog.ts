// "Novedades": the cards of each version in CHANGELOG.md (### 🇪🇸 / ### 🇬🇧 sections), and version order.
import type { Lang } from './i18n'

export interface ChangeItem {
  emoji: string
  title: string
  text: string
}

/** 0.10.0 > 0.9.2 */
export const isNewer = (a: string, b: string): boolean => {
  const pa = a.split('.').map(Number)
  const pb = b.split('.').map(Number)
  for (let i = 0; i < 3; i++) if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) > (pb[i] || 0)
  return false
}


const EMOJI = /^(\p{Extended_Pictographic}(️|‍\p{Extended_Pictographic})*)\s*/u
const plain = (s: string): string => s.replace(/\*\*(.+?)\*\*/g, '$1').replace(/\*(.+?)\*/g, '$1').replace(/`([^`]+)`/g, '$1')

/** One version's cards in that language: each top bullet, its sub-bullets folded into the text. */
export function changesFor(raw: string, version: string, lang: Lang): ChangeItem[] {
  const text = raw.replace(/\r\n/g, '\n')
  const start = text.indexOf(`## ${version}\n`)
  if (start < 0) return []
  const next = text.indexOf('\n## ', start + 4)
  const section = text.slice(start, next < 0 ? undefined : next)
  const from = section.indexOf(lang === 'es' ? '### 🇪🇸' : '### 🇬🇧')
  if (from < 0) return []
  const end = section.indexOf('\n### ', from + 4)
  const items: ChangeItem[] = []
  for (const l of section.slice(from, end < 0 ? undefined : end).split('\n')) {
    if (l.startsWith('- ')) {
      let line = l.slice(2).trim()
      const e = EMOJI.exec(line)
      if (e) line = line.slice(e[0].length)
      const m = /^\*\*(.+?)\*\*[:.,]?\s*(.*)$/.exec(line)
      const rest = plain(m?.[2] ?? '').replace(/^[\s,;:.–-]+/, '')
      items.push({ emoji: e?.[1] ?? '✨', title: plain(m ? m[1] : line), text: rest.charAt(0).toUpperCase() + rest.slice(1) })
    } else if (/^\s+- /.test(l) && items.length) {
      const it = items[items.length - 1]
      const sub = plain(l.trim().slice(2))
      it.text = it.text ? `${it.text.replace(/:$/, '')}${it.text.endsWith(':') ? ': ' : ' · '}${sub}` : sub
    }
  }
  return items
}

/** Everything from after `from` up to `to`, newest first. */
export function changesSince(raw: string, from: string, to: string, lang: Lang): ChangeItem[] {
  const versions = [...raw.matchAll(/^## (\d+\.\d+\.\d+)\s*$/gm)].map((m) => m[1])
  return versions.filter((v) => isNewer(v, from) && !isNewer(v, to)).flatMap((v) => changesFor(raw, v, lang))
}

