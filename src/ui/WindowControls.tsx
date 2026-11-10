import { getCurrentWindow, type Window } from '@tauri-apps/api/window'
import { native } from '../core/api'
import { useT } from '../core/i18n'
import * as Ico from './shell/icons'

/** macOS keeps its native traffic lights (titleBarStyle Overlay): no custom buttons there,
 *  and `html[data-platform=mac]` lets skins leave room for them on the left. */
export const isMac = /Mac/.test(navigator.userAgent)

/** Spread on the frameless title bar and on any child that should also drag the window. */
export const dragRegion = { 'data-tauri-drag-region': '' } as const

const act = (f: (w: Window) => Promise<void>) => () => {
  if (native) void f(getCurrentWindow())
}

/** Minimize / maximize / close (mini: no maximize). Markup: .wc-group > button.wc(.wc-min|.wc-max|.wc-close) > svg.ico */
export function WindowControls({ mini }: { mini?: boolean }) {
  const t = useT()
  if (isMac) return null
  return (
    <div className="wc-group">
      <button className="wc wc-min" onClick={act((w) => w.minimize())} aria-label={t('titlebar.minimize')} title={t('titlebar.minimize')}>
        <Ico.Minimize />
      </button>
      {!mini && (
        <button className="wc wc-max" onClick={act((w) => w.toggleMaximize())} aria-label={t('titlebar.maximize')} title={t('titlebar.maximize')}>
          <Ico.Maximize />
        </button>
      )}
      <button className="wc wc-close" onClick={act((w) => w.close())} aria-label={t('titlebar.close')} title={t('titlebar.close')}>
        <Ico.Close />
      </button>
    </div>
  )
}
