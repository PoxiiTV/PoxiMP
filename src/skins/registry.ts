// Skins are themes only: every skin uses the same shell (src/ui/shell) and changes its colours,
// fonts, shapes and a few decorative touches through CSS (theme.css, keyed on html[data-skin]).
import type { Variant } from '../core/store'
import './caraa/theme.css'
import './pletina/theme.css'
import './portada/theme.css'
import './fosforo/theme.css'
import './ruido/theme.css'
import './stream/theme.css'

/** Name = i18n key skins.<id>. The first variant is the one a skin opens with. */
export interface Skin {
  id: string
  variants: Variant[]
  /** Swatch for the picker in Ajustes → Apariencia */
  preview: string
  /** Colours taken from the playing track's cover */
  cover?: boolean
  /** Small live VU meter next to the tools */
  vu?: boolean
}

export const skins: Skin[] = [
  { id: 'caraa', variants: ['dark'], cover: true, preview: 'linear-gradient(120deg, #3a1630, #170a12)' },
  { id: 'pletina', variants: ['dark'], vu: true, preview: 'linear-gradient(#26272b, #121214)' },
  { id: 'portada', variants: ['light', 'dark'], preview: 'linear-gradient(90deg, #f1ece1 50%, #15120e 50%)' },
  { id: 'fosforo', variants: ['dark'], preview: 'repeating-linear-gradient(#100f0c 0 3px, #1c1a14 3px 4px)' },
  { id: 'ruido', variants: ['light'], preview: 'linear-gradient(90deg, #ffe03a 33%, #ff5da2 33% 66%, #f5eedf 66%)' },
  { id: 'stream', variants: ['dark'], preview: 'linear-gradient(120deg, #1c1c1f, #070708 70%, #d4f53c 70%)' }
]

export const findSkin = (id: string): Skin => skins.find((s) => s.id === id) ?? skins[0]
