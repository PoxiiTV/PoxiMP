// The shared screens (queue, folders, playlists, radio and mixes, lyrics, stats, Wrapped, album /
// artist pages). The shell renders <SharedView> in its content area for every tab but Biblioteca.
import type { ComponentType } from 'react'
import type { View } from '../../core/store'
import type { Track } from '../../core/types'
import { Browse } from './Browse'
import { BadgeSlot } from './common'
import { Folders } from './Folders'
import { Lyrics } from './Lyrics'
import { Playlists } from './Playlists'
import { Queue } from './Queue'
import { Radio } from './Radio'
import { Stats } from './Stats'
import { Wrapped } from './Wrapped'
import './views-base.css'

const VIEWS: Partial<Record<View, ComponentType>> = { folders: Folders, queue: Queue, lyrics: Lyrics, playlists: Playlists, radio: Radio, stats: Stats, wrapped: Wrapped, browse: Browse }

export function SharedView({ view, badge }: { view: View; badge?: ComponentType<{ track: Track }> }) {
  const V = VIEWS[view]
  if (!V) return null
  return (
    <BadgeSlot.Provider value={badge ?? null}>
      <V />
    </BadgeSlot.Provider>
  )
}
