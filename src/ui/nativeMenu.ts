// Native OS context menus (Tauri 2 menu API) for popups that open from the player: they may leave
// the window, so nothing gets cut in the 150 px mini player. Only inside Tauri (`native`).
import { LogicalPosition } from '@tauri-apps/api/dpi'
import { Menu, type CheckMenuItemOptions, type MenuItemOptions, type PredefinedMenuItemOptions, type SubmenuOptions } from '@tauri-apps/api/menu'
import { uiZoom } from '../core/window'
import { isMac } from './WindowControls'

/** null = separator. `checked` makes it a check item (✓ = the current choice); no `run` and no
 *  `items` = a greyed heading. */
export type Entry = { text: string; run?: () => void; checked?: boolean; items?: Entry[] } | null

type Opt = MenuItemOptions | CheckMenuItemOptions | SubmenuOptions | PredefinedMenuItemOptions

// Windows reads "&" as an access-key marker: double it so titles like "Simon & Garfunkel" show as is
const label = (s: string): string => (isMac ? s : s.replace(/&/g, '&&'))

const opt = (e: Entry): Opt => {
  if (!e) return { item: 'Separator' }
  if (e.items) return { text: label(e.text), items: e.items.map(opt) }
  const base: MenuItemOptions = { text: label(e.text), enabled: !!e.run, ...(e.run ? { action: e.run } : {}) }
  return e.checked === undefined ? base : { ...base, checked: e.checked }
}

/** Only one context menu can be open: the previous one is freed when the next one is built. */
let last: Menu | null = null

/** Shows the menu under `at` (or at the pointer). */
export async function popupMenu(entries: Entry[], at?: HTMLElement): Promise<void> {
  const menu = await Menu.new({ items: entries.map(opt) })
  void last?.close().catch(() => {})
  last = menu
  const r = at?.getBoundingClientRect()
  await menu.popup(r ? new LogicalPosition(r.left * uiZoom(), (r.bottom + 2) * uiZoom()) : undefined)
}
