// Cola: the queue in play order (shuffle applied), now-playing marker, drag / Alt+arrows to reorder,
// remove, clear. Refetches on "player:queue".
import { useCallback, useEffect } from 'react'
import { api, on } from '../../core/api'
import { usePaged } from '../../core/hooks'
import { useT } from '../../core/i18n'
import { useStore } from '../../core/store'
import { toast } from '../TrackMenu'
import { Empty, TrackList, ViewHead } from './common'

export function Queue() {
  const t = useT()
  const shuffle = useStore((s) => s.player.shuffle)
  const fetch = useCallback((offset: number, limit: number) => api.queueGet(offset, limit), [])
  const q = usePaged(fetch, 'queue')
  const { reload, edit } = q
  useEffect(() => on('player:queue', reload), [reload])
  // Track changes move the marker without a queue event
  const trackId = useStore((s) => s.track?.id)
  useEffect(() => reload(), [trackId, reload])

  const cur = q.last?.index ?? -1
  const err = (): void => {
    toast(t('err.unknown'))
    reload()
  }

  const move = (from: number, to: number): void => {
    edit((rows) => {
      const [r] = rows.splice(from, 1)
      rows.splice(to, 0, r)
      return rows
    })
    api.queueMove(from, to).then(reload, err)
  }
  const remove = (i: number): void => {
    edit((rows) => (rows.splice(i, 1), rows))
    api.queueRemove([i]).then(reload, err)
  }
  const play = (i: number): void => {
    if (i === cur && useStore.getState().player.status !== 'playing') return void api.toggle()
    if (i !== cur) api.queueJump(i).catch(err)
  }

  const count = q.total === 1 ? t('library.song') : t('library.songs', { n: q.total })
  const left = cur >= 0 ? Math.max(0, q.total - cur - 1) : q.total

  return (
    <div className="vw vw-queue">
      <ViewHead
        kicker={shuffle ? t('views.queue.shuffled') : t('views.queue.kicker')}
        title={t('nav.queue')}
        meta={q.ready && q.total > 0 && <span>{count} · {t('views.queue.left', { n: left })}</span>}
      >
        <button className="btn" disabled={q.total <= 1} onClick={() => void api.queueClear().then(reload, err)}>
          {t('views.queue.clear')}
        </button>
      </ViewHead>
      {q.ready && q.total === 0 ? (
        <Empty title={t('views.queue.empty')} hint={t('views.queue.emptyHint')} />
      ) : (
        <TrackList
          total={q.total}
          rows={q.rows}
          ensureRange={q.ensureRange}
          label={t('nav.queue')}
          playingIndex={cur}
          playedBefore={cur}
          reveal={cur}
          onPlay={play}
          onMove={move}
          onRemove={remove}
          menuExtra={(i: number) => [{ label: t('views.queue.remove'), run: () => remove(i), danger: true }]}
        />
      )}
      <p className="vw-foot">{t('views.queue.hint')}</p>
    </div>
  )
}
