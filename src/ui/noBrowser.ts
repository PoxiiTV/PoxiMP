/** No browser behaviour in any window: no context menu, no browser shortcuts (print, find, reload, save page…). */
export function blockBrowserUi(): void {
  window.addEventListener('contextmenu', (e) => {
    if (!import.meta.env.DEV || !e.shiftKey) e.preventDefault()
  })
  window.addEventListener('keydown', (e) => {
    const k = e.key.toLowerCase()
    if (e.key === 'F5' || e.key === 'F3' || e.key === 'F7' || (e.ctrlKey && ['r', 'p', 's', 'g', 'u', 'j', 'h', 'o', 'd', 'f'].includes(k))) e.preventDefault()
  })
}
