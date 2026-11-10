// Unstyled, accessible controls. Skins style them through the class names and the CSS
// variables they set (--v for Slider, --p for Seekbar, as percentages).
import { useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react'
import { pct } from './quality'

type Css = CSSProperties & Record<`--${string}`, string>

/** Pointer drag over an element → fraction 0..1 along its width. */
function useDrag(onMove: (f: number) => void, onEnd?: (f: number) => void) {
  const last = useRef(0)
  const frac = (e: PointerEvent<HTMLElement>): number => {
    const r = e.currentTarget.getBoundingClientRect()
    return (last.current = r.width ? Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) : 0)
  }
  return {
    onPointerDown(e: PointerEvent<HTMLElement>) {
      if (e.button !== 0) return
      e.currentTarget.setPointerCapture(e.pointerId)
      onMove(frac(e))
    },
    onPointerMove(e: PointerEvent<HTMLElement>) {
      if (e.currentTarget.hasPointerCapture(e.pointerId)) onMove(frac(e))
    },
    onPointerUp(e: PointerEvent<HTMLElement>) {
      if (!e.currentTarget.hasPointerCapture(e.pointerId)) return
      e.currentTarget.releasePointerCapture(e.pointerId)
      onEnd?.(last.current)
    }
  }
}

/** Arrow/Page/Home/End keys for a value in [0, max]. */
function keyStep(e: KeyboardEvent, value: number, max: number, step: number, big: number, set: (v: number) => void): void {
  const map: Record<string, number> = {
    ArrowRight: value + step,
    ArrowUp: value + step,
    ArrowLeft: value - step,
    ArrowDown: value - step,
    PageUp: value + big,
    PageDown: value - big,
    Home: 0,
    End: max
  }
  if (!(e.key in map)) return
  e.preventDefault()
  set(Math.max(0, Math.min(max, map[e.key])))
}

interface SliderProps {
  /** 0..1 */
  value: number
  onChange: (v: number) => void
  label: string
  valueText?: string
  className?: string
  /** Set = cannot be moved; the text says why (tooltip). */
  locked?: string
}

/** Locked controls look it: dimmed, no pointer feedback. Inline so every skin gets it. */
export const lockedStyle = { opacity: 0.45, cursor: 'not-allowed' } as const

/** Horizontal slider 0..1: `<div class="slider" style="--v:72%"><i class="slider-thumb"/></div>` */
export function Slider({ value, onChange, label, valueText, className = '', locked }: SliderProps) {
  const drag = useDrag(onChange)
  return (
    <div
      className={`slider ${className} ${locked ? 'locked' : ''}`}
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(value * 100)}
      aria-valuetext={valueText}
      aria-disabled={locked ? true : undefined}
      title={locked}
      style={{ '--v': pct(value), ...(locked && lockedStyle) } as Css}
      onKeyDown={(e) => !locked && keyStep(e, value, 1, 0.05, 0.2, onChange)}
      {...(locked ? {} : drag)}
    >
      <i className="slider-thumb" />
    </div>
  )
}

interface SeekbarProps {
  /** Seconds */
  value: number
  max: number
  onSeek: (secs: number) => void
  label: string
  valueText?: string
  /** Layers drawn under the head (the waveform); they can use --p for the played part */
  children?: ReactNode
  className?: string
}

/** Seek bar: drag shows the target and seeks on release; arrows ±5 s, Page ±30 s.
 *  Markup: .seekbar[--p] > {children}, .head */
export function Seekbar({ value, max, onSeek, label, valueText, children, className = '' }: SeekbarProps) {
  const [drag, setDrag] = useState<number | null>(null)
  const handlers = useDrag(setDrag, (f) => {
    setDrag(null)
    if (max > 0) onSeek(f * max)
  })
  const f = drag ?? (max > 0 ? value / max : 0)
  return (
    <div
      className={`seekbar ${drag !== null ? 'dragging' : ''} ${className}`}
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(f * max)}
      aria-valuetext={valueText}
      aria-disabled={max <= 0}
      style={{ '--p': pct(f) } as Css}
      onKeyDown={(e) => max > 0 && keyStep(e, value, max, 5, 30, onSeek)}
      {...handlers}
    >
      {children}
      <div className="head" />
    </div>
  )
}
