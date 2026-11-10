import { Fragment, useEffect, useLayoutEffect, useRef, useState, type HTMLAttributes, type ReactNode } from 'react'

interface Props extends Omit<HTMLAttributes<HTMLDivElement>, 'children' | 'onScroll'> {
  count: number
  /** Fixed row height in px */
  rowHeight: number
  children: (index: number) => ReactNode
  /** Visible rows (overscan included) changed: load them */
  onRange?: (start: number, end: number) => void
  /** Row to keep in view (keyboard selection) */
  active?: number
  overscan?: number
}

/** Fixed-height virtual list: only the rows in view are in the DOM. The element itself scrolls. */
export function VirtualList({ count, rowHeight, children, onRange, active = -1, overscan = 6, style, ...rest }: Props) {
  const ref = useRef<HTMLDivElement>(null)
  const [top, setTop] = useState(0)
  const [height, setHeight] = useState(0)

  useLayoutEffect(() => {
    const el = ref.current!
    const ro = new ResizeObserver(() => setHeight(el.clientHeight))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const start = Math.max(0, Math.floor(top / rowHeight) - overscan)
  const end = Math.min(count - 1, Math.ceil((top + height) / rowHeight) + overscan)

  useEffect(() => {
    if (count > 0) onRange?.(start, end)
  }, [start, end, count, onRange])

  useEffect(() => {
    const el = ref.current
    if (!el || active < 0) return
    const y = active * rowHeight
    if (y < el.scrollTop) el.scrollTop = y
    else if (y + rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = y + rowHeight - el.clientHeight
  }, [active, rowHeight])

  const rows: ReactNode[] = []
  for (let i = start; i <= end; i++) rows.push(<Fragment key={i}>{children(i)}</Fragment>)

  return (
    <div ref={ref} {...rest} style={{ ...style, overflowY: 'auto' }} onScroll={(e) => setTop(e.currentTarget.scrollTop)}>
      <div style={{ height: count * rowHeight, position: 'relative' }}>
        <div style={{ position: 'absolute', inset: '0 0 auto', transform: `translateY(${start * rowHeight}px)` }}>{rows}</div>
      </div>
    </div>
  )
}
