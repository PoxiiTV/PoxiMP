import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { App } from './App'
import { native } from './core/api'
import { boot } from './core/hooks'
import { restoreWindow } from './core/window'
import { blockBrowserUi } from './ui/noBrowser'
import { isMac } from './ui/WindowControls'

blockBrowserUi()
document.documentElement.dataset.platform = isMac ? 'mac' : 'win'
boot()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
)

// The window starts hidden: back to its last size and mode, then shown once painted (no white flash).
// macOS: WebKit runs no animation frames in a hidden window, so it waits for the next task instead.
const painted = isMac ? (f: () => void) => setTimeout(f) : requestAnimationFrame
if (native) painted(() => void restoreWindow().finally(() => getCurrentWindow().show()))
