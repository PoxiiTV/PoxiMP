// Every icon of the app: Material Symbols Rounded, weight 400 (Google, Apache-2.0 —
// node_modules/@material-symbols/svg-400/LICENSE). Transport glyphs (prev/play/pause/next) are the
// filled ones, everything else outlined. Imported at build time (?raw), so only these ship and the
// app stays offline. One 960-unit viewBox for all: sizes in CSS stay optically consistent.
// Skins never swap them: they only colour them (currentColor).
import type { ReactNode } from 'react'
import add from '@material-symbols/svg-400/rounded/add.svg?raw'
import arrowBack from '@material-symbols/svg-400/rounded/arrow_back.svg?raw'
import barChart from '@material-symbols/svg-400/rounded/bar_chart.svg?raw'
import bedtime from '@material-symbols/svg-400/rounded/bedtime.svg?raw'
import check from '@material-symbols/svg-400/rounded/check.svg?raw'
import chevronLeft from '@material-symbols/svg-400/rounded/chevron_left.svg?raw'
import chevronRight from '@material-symbols/svg-400/rounded/chevron_right.svg?raw'
import close from '@material-symbols/svg-400/rounded/close.svg?raw'
import closeFullscreen from '@material-symbols/svg-400/rounded/close_fullscreen.svg?raw'
import cropSquare from '@material-symbols/svg-400/rounded/crop_square.svg?raw'
import download from '@material-symbols/svg-400/rounded/download.svg?raw'
import dragIndicator from '@material-symbols/svg-400/rounded/drag_indicator.svg?raw'
import folder from '@material-symbols/svg-400/rounded/folder.svg?raw'
import home from '@material-symbols/svg-400/rounded/home.svg?raw'
import info from '@material-symbols/svg-400/rounded/info.svg?raw'
import keep from '@material-symbols/svg-400/rounded/keep.svg?raw'
import keepOff from '@material-symbols/svg-400/rounded/keep_off.svg?raw'
import keyboard from '@material-symbols/svg-400/rounded/keyboard.svg?raw'
import libraryMusic from '@material-symbols/svg-400/rounded/library_music.svg?raw'
import list from '@material-symbols/svg-400/rounded/list.svg?raw'
import lyrics from '@material-symbols/svg-400/rounded/lyrics.svg?raw'
import mobile from '@material-symbols/svg-400/rounded/mobile.svg?raw'
import musicNote from '@material-symbols/svg-400/rounded/music_note.svg?raw'
import openInFull from '@material-symbols/svg-400/rounded/open_in_full.svg?raw'
import palette from '@material-symbols/svg-400/rounded/palette.svg?raw'
import pauseFill from '@material-symbols/svg-400/rounded/pause-fill.svg?raw'
import person from '@material-symbols/svg-400/rounded/person.svg?raw'
import playArrowFill from '@material-symbols/svg-400/rounded/play_arrow-fill.svg?raw'
import playCircle from '@material-symbols/svg-400/rounded/play_circle.svg?raw'
import playlistPlay from '@material-symbols/svg-400/rounded/playlist_play.svg?raw'
import queueMusic from '@material-symbols/svg-400/rounded/queue_music.svg?raw'
import radio from '@material-symbols/svg-400/rounded/radio.svg?raw'
import remove from '@material-symbols/svg-400/rounded/remove.svg?raw'
import repeat from '@material-symbols/svg-400/rounded/repeat.svg?raw'
import repeatOne from '@material-symbols/svg-400/rounded/repeat_one.svg?raw'
import search from '@material-symbols/svg-400/rounded/search.svg?raw'
import settings from '@material-symbols/svg-400/rounded/settings.svg?raw'
import shuffle from '@material-symbols/svg-400/rounded/shuffle.svg?raw'
import skipNextFill from '@material-symbols/svg-400/rounded/skip_next-fill.svg?raw'
import skipPreviousFill from '@material-symbols/svg-400/rounded/skip_previous-fill.svg?raw'
import speaker from '@material-symbols/svg-400/rounded/speaker.svg?raw'
import star from '@material-symbols/svg-400/rounded/star.svg?raw'
import starShine from '@material-symbols/svg-400/rounded/star_shine.svg?raw'
import tune from '@material-symbols/svg-400/rounded/tune.svg?raw'
import graphicEq from '@material-symbols/svg-400/rounded/graphic_eq.svg?raw'
import volumeDown from '@material-symbols/svg-400/rounded/volume_down.svg?raw'
import volumeOff from '@material-symbols/svg-400/rounded/volume_off.svg?raw'
import volumeUp from '@material-symbols/svg-400/rounded/volume_up.svg?raw'

/** `<svg class="ico">` from a Material Symbols file (one path, fill = currentColor). */
function icon(raw: string) {
  const d = /\sd="([^"]+)"/.exec(raw)?.[1] ?? ''
  return function Icon() {
    return (
      <svg className="ico" viewBox="0 -960 960 960" fill="currentColor" aria-hidden="true">
        <path d={d} />
      </svg>
    )
  }
}

// ---------- general ----------
export const Search = icon(search)
export const Close = icon(close)
export const Gear = icon(settings)
export const Tick = icon(check)
export const Plus = icon(add)
export const Back = icon(arrowBack)
export const ChevronLeft = icon(chevronLeft)
export const ChevronRight = icon(chevronRight)
export const Drag = icon(dragIndicator)
export const Music = icon(musicNote)
export const Person = icon(person)
export const List = icon(list)
export const Sparkle = icon(starShine)

// ---------- window ----------
const Shrink = icon(closeFullscreen)
const Expand = icon(openInFull)
/** Mini ↔ full: in mini it expands, in full it shrinks */
export const Mini = ({ full }: { full?: boolean }) => (full ? <Expand /> : <Shrink />)
export const Minimize = icon(remove)
export const Maximize = icon(cropSquare)

// ---------- transport (filled) ----------
export const Play = icon(playArrowFill)
export const Pause = icon(pauseFill)
export const Prev = icon(skipPreviousFill)
export const Next = icon(skipNextFill)
const RepeatAll = icon(repeat)
const RepeatOne = icon(repeatOne)
export const Repeat = ({ one }: { one?: boolean }) => (one ? <RepeatOne /> : <RepeatAll />)
export const Shuffle = icon(shuffle)
const VolOff = icon(volumeOff)
const VolDown = icon(volumeDown)
const VolUp = icon(volumeUp)
/** Speaker whose waves follow the level (0..1): none (muted), one, two */
export const Volume = ({ level }: { level: number }) => (level <= 0 ? <VolOff /> : level < 0.5 ? <VolDown /> : <VolUp />)

// ---------- tools ----------
export const Eq = icon(tune)
export const Lyrics = icon(lyrics)
export const Timer = icon(bedtime)
const Keep = icon(keep)
const KeepOff = icon(keepOff)
export const Pin = ({ on }: { on: boolean }) => (on ? <Keep /> : <KeepOff />)
export const Phone = icon(mobile)

// ---------- tabs ----------
export const TAB: Record<string, () => ReactNode> = {
  queue: icon(queueMusic),
  library: icon(libraryMusic),
  folders: icon(folder),
  playlists: icon(playlistPlay),
  radio: icon(radio),
  lyrics: Lyrics,
  stats: icon(barChart)
}

// ---------- settings sections ----------
export const SECTION: Record<string, () => ReactNode> = {
  general: icon(home),
  appearance: icon(palette),
  playback: icon(playCircle),
  output: icon(speaker),
  eq: Eq,
  fx: icon(graphicEq),
  library: TAB.library,
  quality: icon(star),
  lyrics: Lyrics,
  remote: Phone,
  keys: icon(keyboard),
  import: icon(download),
  about: icon(info)
}
