import es from './es'
import en from './en'
import { useStore } from '../store'

export type Lang = 'es' | 'en'

const dicts: Record<Lang, unknown> = { es, en }

function lookup(dict: unknown, key: string): unknown {
  // Los códigos de error de Rust ya llevan punto (`err.vault.wrongPassword` → `err['vault.wrongPassword']`)
  if (key.startsWith('err.')) return ((dict as Record<string, Record<string, unknown> | undefined>).err ?? {})[key.slice(4)]
  return key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], dict)
}

export function systemLang(): Lang {
  return navigator.language.toLowerCase().startsWith('es') ? 'es' : 'en'
}

/** Traduce con fallback a español; `{param}` se sustituye. */
export function translate(lang: Lang, key: string, params?: Record<string, string | number>): string {
  const v = lookup(dicts[lang], key) ?? lookup(es, key)
  const raw = typeof v === 'string' ? v : key
  return params ? raw.replace(/\{(\w+)\}/g, (m, p: string) => (p in params ? String(params[p]) : m)) : raw
}

export function translateList(lang: Lang, key: string): string[] {
  const v = lookup(dicts[lang], key) ?? lookup(es, key)
  return Array.isArray(v) ? (v as string[]) : []
}

export function currentLang(): Lang {
  const l = useStore.getState().config?.lang
  return l ?? systemLang()
}

export function t(key: string, params?: Record<string, string | number>): string {
  return translate(currentLang(), key, params)
}

/** En componentes: se vuelve a pintar al cambiar de idioma. */
export function useT(): typeof t {
  useStore((s) => s.config?.lang)
  return t
}

/** Mensaje para un error de Rust: código suelto (`'network_error'`) o `{ code, detail }`. */
export function errorText(e: unknown): string {
  const code = typeof e === 'string' ? e : typeof e === 'object' && e && 'code' in e ? String((e as { code: string }).code) : 'unknown'
  const detail = typeof e === 'object' && e && 'detail' in e ? String((e as { detail?: string }).detail ?? '') : ''
  const msg = translate(currentLang(), `err.${code}`, { d: detail })
  return msg === `err.${code}` ? translate(currentLang(), 'err.unknown') : msg
}
