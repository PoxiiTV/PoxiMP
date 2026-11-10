// An album or an artist opened from the search bar: its tracks (album order), play, radio.
import { useCallback, useMemo } from 'react'
import { api } from '../../core/api'
import { total as fmtTotal } from '../../core/format'
import { usePaged } from '../../core/hooks'
import { errorText, useT } from '../../core/i18n'
import { useStore } from '../../core/store'
import type { ViewQuery } from '../../core/types'
import { toast } from '../TrackMenu'
import { Empty, TrackList, ViewHead } from './common'

export function Browse() {
  const t = useT()
  const b = useStore((s) => s.browse)
  const q = useMemo<ViewQuery | null>(() => {
    if (!b) return null
    return b.kind === 'album' ? { sort: 'album', filter: '', albumKey: b.id } : { sort: 'album', filter: '', artistId: Number(b.id) }
  }, [b])
  const key = b ? `${b.kind}:${b.id}` : 'none'
  const fetch = useCallback((offset: number, limit: number) => (q ? api.page({ ...q, offset, limit }) : Promise.resolve({ total: 0, duration: 0, counts: { ok: 0, doubt: 0, pending: 0 }, tracks: [] })), [q])
  const pg = usePaged(fetch, key)
  if (!b || !q) return <div className="vw" />

  const err = (e: unknown): void => toast(errorText(e))
  const first = pg.rows[0]
  const n = pg.total
  return (
    <div className="vw vw-browse">
      <ViewHead
        kicker={t(`views.browse.${b.kind}`)}
        title={b.title}
        meta={pg.ready && <span>{n === 1 ? t('library.song') : t('library.songs', { n })}{pg.last ? ` · ${fmtTotal(pg.last.duration)}` : ''}</span>}
      >
        <button className="btn primary" disabled={!n} onClick={() => void api.playView(q).catch(err)}>
          {t('player.play')}
        </button>
        <button
          className="btn"
          disabled={!first}
          onClick={() => first && void api.radioStart(first.id).then(() => toast(t('views.menu.radioDone', { title: b.title })), err)}
        >
          {t('views.browse.radio')}
        </button>
      </ViewHead>
      {pg.ready && n === 0 ? (
        <Empty title={t('views.browse.none')} />
      ) : (
        <TrackList total={n} rows={pg.rows} ensureRange={pg.ensureRange} label={b.title} album={b.kind === 'artist'} onPlay={(_, tr) => void api.playView(q, tr.id).catch(err)} />
      )}
    </div>
  )
}
