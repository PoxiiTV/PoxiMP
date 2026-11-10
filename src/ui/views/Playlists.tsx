// Listas: index of manual + smart playlists (create, import), a playlist's page (play, rename, delete,
// export, reorder/remove for manual ones) and the smart rules editor.
import { useCallback, useEffect, useState, type FormEvent } from 'react'
import { api } from '../../core/api'
import { total as fmtTotal } from '../../core/format'
import { usePaged } from '../../core/hooks'
import { errorText, useT } from '../../core/i18n'
import { setState, useStore } from '../../core/store'
import type { Playlist, SmartNode, SmartRules } from '../../core/types'
import { toast } from '../TrackMenu'
import * as Ico from '../shell/icons'
import { Empty, TrackList, ViewHead, fileName, saveFile } from './common'
import { ImportButtons } from './Import'
import { SmartEditor, NEW_RULES } from './SmartEditor'

const countRules = (n: SmartNode): number => ('all' in n ? n.all.reduce((s, c) => s + countRules(c), 0) : 'any' in n ? n.any.reduce((s, c) => s + countRules(c), 0) : 1)

/** Edit target: an existing smart playlist or a new one (id null). */
export interface Editing {
  id: number | null
  name: string
  rules: SmartRules
}

export function Playlists() {
  const id = useStore((s) => s.playlist)
  const [lists, setLists] = useState<Playlist[] | null>(null)
  const [editing, setEditing] = useState<Editing | null>(null)
  // Bumped after saving rules so the open page refetches its (re-evaluated) tracks
  const [rev, setRev] = useState(0)
  const reload = useCallback(() => {
    api.playlists().then(setLists, () => setLists([]))
  }, [])
  useEffect(reload, [reload, id])

  const open = lists?.find((p) => p.id === id) ?? null
  if (editing)
    return (
      <SmartEditor
        editing={editing}
        onClose={() => setEditing(null)}
        onSaved={(pid) => {
          setEditing(null)
          setRev((r) => r + 1)
          setState({ playlist: pid })
          reload()
        }}
      />
    )
  if (id !== null && !lists) return <div className="vw" />
  if (id !== null && open) return <Page key={`${open.id}:${rev}`} list={open} onChanged={reload} onEdit={setEditing} />
  return <Index lists={lists} onChanged={reload} onNewSmart={() => setEditing({ id: null, name: '', rules: NEW_RULES })} />
}

function Index({ lists, onChanged, onNewSmart }: { lists: Playlist[] | null; onChanged: () => void; onNewSmart: () => void }) {
  const t = useT()
  const [naming, setNaming] = useState(false)
  const manual = lists?.filter((p) => p.kind === 'manual') ?? []
  const smart = lists?.filter((p) => p.kind === 'smart') ?? []

  const create = (e: FormEvent<HTMLFormElement>): void => {
    e.preventDefault()
    const name = String(new FormData(e.currentTarget).get('name') ?? '').trim()
    if (!name) return setNaming(false)
    api.playlistCreate(name.slice(0, 200)).then((p) => {
      setNaming(false)
      setState({ playlist: p.id })
    }, (er) => toast(errorText(er)))
  }

  return (
    <div className="vw vw-lists">
      <ViewHead kicker={t('views.playlists.kicker')} title={t('nav.playlists')} meta={lists && <span>{t('views.playlists.count', { n: lists.length })}</span>}>
        <button className="btn" onClick={() => setNaming(true)}>
          {t('views.playlists.new')}
        </button>
        <button className="btn primary" onClick={onNewSmart}>
          {t('views.playlists.newSmart')}
        </button>
      </ViewHead>
      <div className="vw-scrollpane">
        <section className="vw-sec">
          <h3 className="vw-sec-t">{t('views.playlists.manual')}</h3>
          {naming && (
            <form className="vw-inline" onSubmit={create}>
              <input name="name" className="vw-input" autoFocus maxLength={200} placeholder={t('views.playlists.namePlaceholder')} aria-label={t('views.playlists.name')} onKeyDown={(e) => e.key === 'Escape' && setNaming(false)} />
              <button className="btn primary" type="submit">
                {t('views.playlists.create')}
              </button>
              <button className="btn" type="button" onClick={() => setNaming(false)}>
                {t('eq.cancel')}
              </button>
            </form>
          )}
          {lists && !manual.length && !naming && <p className="vw-hint">{t('views.playlists.noManual')}</p>}
          <ul className="vw-cards">{manual.map((p) => <Card key={p.id} p={p} />)}</ul>
        </section>
        <section className="vw-sec">
          <h3 className="vw-sec-t">{t('views.playlists.smart')}</h3>
          <p className="vw-hint">{t('views.playlists.smartHint')}</p>
          <ul className="vw-cards">{smart.map((p) => <Card key={p.id} p={p} />)}</ul>
        </section>
        <section className="vw-sec">
          <h3 className="vw-sec-t">{t('views.import.title')}</h3>
          <p className="vw-hint">{t('views.playlists.importHint')}</p>
          <ImportButtons only={['m3u', 'aimp']} onDone={onChanged} />
        </section>
      </div>
    </div>
  )
}

function Card({ p }: { p: Playlist }) {
  const t = useT()
  return (
    <li>
      <button className={`vw-card k-${p.kind}`} onClick={() => setState({ playlist: p.id })}>
        <span className="vw-card-ico" aria-hidden="true">
          {p.kind === 'smart' ? (
            <Ico.Sparkle />
          ) : (
            <Ico.List />
          )}
        </span>
        <span className="vw-card-t">
          <b>{p.name}</b>
          <span>
            {p.kind === 'smart' ? t('views.playlists.smartOne') : t('views.playlists.manualOne')} · {p.trackCount === 1 ? t('library.song') : t('library.songs', { n: p.trackCount })}
          </span>
        </span>
      </button>
    </li>
  )
}

function Page({ list, onChanged, onEdit }: { list: Playlist; onChanged: () => void; onEdit: (e: Editing) => void }) {
  const t = useT()
  const [renaming, setRenaming] = useState(false)
  const [confirm, setConfirm] = useState(false)
  const fetch = useCallback((offset: number, limit: number) => api.playlistPage(list.id, offset, limit), [list.id])
  const pg = usePaged(fetch, `pl${list.id}`)
  const { reload, edit } = pg
  const manual = list.kind === 'manual'
  const err = (e: unknown): void => {
    toast(errorText(e))
    reload()
  }
  const done = (): void => {
    reload()
    onChanged()
  }

  const move = (from: number, to: number): void => {
    edit((rows) => {
      const [r] = rows.splice(from, 1)
      rows.splice(to, 0, r)
      return rows
    })
    api.playlistMove(list.id, from, to).then(reload, err)
  }
  const remove = (i: number): void => {
    edit((rows) => (rows.splice(i, 1), rows))
    api.playlistRemove(list.id, [i]).then(done, err)
  }
  const rename = (e: FormEvent<HTMLFormElement>): void => {
    e.preventDefault()
    const name = String(new FormData(e.currentTarget).get('name') ?? '').trim()
    setRenaming(false)
    if (name && name !== list.name) api.playlistRename(list.id, name.slice(0, 200)).then(onChanged, err)
  }
  const exportM3u = async (): Promise<void> => {
    const path = await saveFile('M3U', ['m3u8', 'm3u'], `${fileName(list.name)}.m3u8`)
    if (!path) return
    api.exportM3u(list.id, path).then((n) => toast(t('views.playlists.exported', { n })), err)
  }
  const del = (): void => {
    api.playlistDelete(list.id).then(() => {
      setState({ playlist: null })
      onChanged()
    }, err)
  }

  const n = pg.ready ? pg.total : list.trackCount
  const meta = (
    <>
      <span>
        {n === 1 ? t('library.song') : t('library.songs', { n })}
        {pg.last ? ` · ${fmtTotal(pg.last.duration)}` : ''}
      </span>
      {list.rules && (
        <span>
          {t('views.smart.summary', { n: countRules(list.rules) })}
          {list.rules.limit ? ` · ${t('views.smart.max', { n: list.rules.limit })}` : ''}
        </span>
      )}
    </>
  )

  return (
    <div className="vw vw-list">
      <button className="vw-back" onClick={() => setState({ playlist: null })}>
        <Ico.ChevronLeft />
        {t('nav.playlists')}
      </button>
      <ViewHead
        kicker={manual ? t('views.playlists.manualOne') : t('views.playlists.smartOne')}
        title={
          renaming ? (
            <form className="vw-inline" onSubmit={rename}>
              <input name="name" className="vw-input vw-rename" defaultValue={list.name} autoFocus maxLength={200} aria-label={t('views.playlists.name')} onKeyDown={(e) => e.key === 'Escape' && setRenaming(false)} onBlur={(e) => e.currentTarget.form?.requestSubmit()} />
            </form>
          ) : (
            list.name
          )
        }
        meta={meta}
      >
        <button className="btn primary" disabled={!n} onClick={() => void api.playlistPlay(list.id).catch(err)}>
          {t('player.play')}
        </button>
        {list.rules && (
          <button className="btn" onClick={() => onEdit({ id: list.id, name: list.name, rules: list.rules! })}>
            {t('views.smart.edit')}
          </button>
        )}
        <button className="btn" onClick={() => setRenaming(true)}>
          {t('views.playlists.rename')}
        </button>
        <button className="btn" disabled={!n} onClick={() => void exportM3u()}>
          {t('views.playlists.export')}
        </button>
        {confirm ? (
          <span className="vw-confirm" role="group" aria-label={t('views.playlists.deleteAsk')}>
            <button className="btn danger" autoFocus onClick={del}>
              {t('views.playlists.deleteYes')}
            </button>
            <button className="btn" onClick={() => setConfirm(false)}>
              {t('eq.cancel')}
            </button>
          </span>
        ) : (
          <button className="btn" onClick={() => setConfirm(true)}>
            {t('views.playlists.delete')}
          </button>
        )}
      </ViewHead>
      {pg.ready && pg.total === 0 ? (
        <Empty title={t('views.playlists.empty')} hint={manual ? t('views.playlists.emptyHint') : t('views.playlists.emptySmart')}>
          {!manual && (
            <button className="btn" onClick={() => onEdit({ id: list.id, name: list.name, rules: list.rules! })}>
              {t('views.smart.edit')}
            </button>
          )}
        </Empty>
      ) : (
        <TrackList
          total={pg.total}
          rows={pg.rows}
          ensureRange={pg.ensureRange}
          label={list.name}
          onPlay={(i) => void api.playlistPlay(list.id, i).catch(err)}
          onMove={manual ? move : undefined}
          onRemove={manual ? remove : undefined}
          menuExtra={manual ? (i) => [{ label: t('views.playlists.removeFrom'), run: () => remove(i), danger: true }] : undefined}
        />
      )}
      {manual && pg.total > 1 && <p className="vw-foot">{t('views.playlists.hint')}</p>}
    </div>
  )
}
