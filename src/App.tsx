import { useEffect, useLayoutEffect } from 'react'
import { api, on } from './core/api'
import { t } from './core/i18n'
import { useStore } from './core/store'
import { Shell } from './ui/shell/Shell'
import { toast } from './ui/TrackMenu'
import { useUpdates } from './ui/Updater'
import './skins/registry'

/** One shell for every skin; a skin only themes it (html[data-skin], data-variant, data-font).
 *  Switching skins never touches playback. */
export function App() {
  const choice = useStore((s) => s.skin)
  const font = useStore((s) => s.prefs.font)
  const lang = useStore((s) => s.config?.lang)
  // Bit-perfect refused (busy, switched off in Windows) or the chosen device gone: say it once,
  // when it happens (formatChanged is only shown in Ajustes: it is not an error).
  const fallback = useStore((s) => s.output?.fallback)
  useUpdates()
  useEffect(() => {
    if (fallback && fallback !== 'formatChanged') toast(t(`output.toast.${fallback}`), 4500)
  }, [fallback])
  // Files "open with PoxiMP" refused (also those from before the UI was up): one short toast
  useEffect(() => {
    const show = (): void =>
      void api.openRejected().then((list) => {
        if (list.length) toast(list.length > 1 ? t('open.many', { n: list.length }) : t(`open.${list[0].reason}`, { name: list[0].name }), 4500)
      }, () => {})
    show()
    return on('open:rejected', show)
  }, [])
  useLayoutEffect(() => {
    const d = document.documentElement.dataset
    d.skin = choice.id
    d.variant = choice.variant
    d.font = font
  }, [choice, font])
  useEffect(() => {
    document.documentElement.lang = lang ?? (navigator.language.toLowerCase().startsWith('es') ? 'es' : 'en')
  }, [lang])
  return <Shell />
}
