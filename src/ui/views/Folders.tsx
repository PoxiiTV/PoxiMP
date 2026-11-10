// Carpetas: the music folders as a tree (WAI-ARIA tree, roving focus) and the tracks of the selected
// one, optionally with its subfolders. Double-click plays the folder from that track (play_view).
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react'
import { api, native } from '../../core/api'
import { total as fmtTotal } from '../../core/format'
import { removeFolder, usePaged } from '../../core/hooks'
import { useT } from '../../core/i18n'
import { setState, useStore } from '../../core/store'
import type { FolderNode, ViewQuery } from '../../core/types'
import { popupMenu } from '../nativeMenu'
import * as Ico from '../shell/icons'
import { isMenuKey, toast } from '../TrackMenu'
import { Empty, Switch, TrackList, ViewHead } from './common'

const norm = (p: string): string => p.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()

interface Flat {
  node: FolderNode
  level: number
  parent: string | null
}

/** Visible rows of the tree, depth-first. */
function flatten(nodes: FolderNode[], open: Set<string>, level = 1, parent: string | null = null, out: Flat[] = []): Flat[] {
  for (const n of nodes) {
    out.push({ node: n, level, parent })
    if (n.children.length && open.has(n.path)) flatten(n.children, open, level + 1, n.path, out)
  }
  return out
}

/** Chain of nodes from a top folder down to the one holding `path` (deepest prefix match). */
function chainTo(nodes: FolderNode[], path: string): FolderNode[] {
  const want = norm(path)
  for (const n of nodes) {
    const p = norm(n.path)
    if (want === p) return [n]
    if (want.startsWith(`${p}/`)) return [n, ...chainTo(n.children, path)]
  }
  return []
}

export function Folders() {
  const t = useT()
  const folder = useStore((s) => s.folder)
  const [tree, setTree] = useState<FolderNode[] | null>(null)
  const [open, setOpen] = useState<Set<string>>(new Set())
  const [recursive, setRecursive] = useState(true)
  const [focus, setFocus] = useState<string | null>(null)
  const treeRef = useRef<HTMLUListElement>(null)

  // Again after a scan or a folder taken out (the library total changes)
  const libTotal = useStore((s) => s.totals?.total)
  useEffect(() => {
    api.folderTree().then(setTree, () => setTree([]))
  }, [libTotal])

  // Selected folder (or "Mostrar en carpeta"): open its ancestors; nothing selected yet = first folder
  useEffect(() => {
    if (!tree?.length) return
    if (!folder) {
      setState({ folder: tree[0].path })
      return
    }
    const chain = chainTo(tree, folder)
    const last = chain.at(-1)
    // "Mostrar en carpeta" comes from a file path: switch to the tree's own spelling of it
    if (last && last.path !== folder && norm(last.path) === norm(folder)) return setState({ folder: last.path })
    if (chain.length) {
      setOpen((o) => new Set([...o, ...chain.slice(0, -1).map((n) => n.path), ...(chain.length === 1 ? [chain[0].path] : [])]))
      setFocus(chain[chain.length - 1].path)
    }
  }, [tree, folder])

  const rows = useMemo(() => (tree ? flatten(tree, open) : []), [tree, open])
  const selNode = useMemo(() => (tree && folder ? (chainTo(tree, folder).at(-1) ?? null) : null), [tree, folder])
  const exact = selNode && norm(selNode.path) === norm(folder ?? '')
  const name = exact ? selNode.name : (folder ?? '')

  const view: ViewQuery = useMemo(() => ({ sort: 'path', filter: '', folder, recursive }), [folder, recursive])
  const fetch = useCallback((offset: number, limit: number) => api.page({ ...view, offset, limit }), [view])
  const list = usePaged(fetch, `${folder}|${recursive}`)
  const duration = list.last?.duration ?? 0

  const select = (p: string): void => setState({ folder: p })
  /** Right-click (or the Menu key) on a folder: take it out of the library. */
  const menu = (node: FolderNode, at?: HTMLElement): void => {
    if (!native) return
    void popupMenu(
      [
        { text: node.name },
        {
          text: t('views.folders.remove'),
          run: () =>
            void removeFolder(node.path)
              .then(() => api.folderTree())
              .then(
                (next) => {
                  // The selected folder went with it: pick the first one left
                  if (folder && (norm(folder) + '/').startsWith(norm(node.path) + '/')) setState({ folder: next[0]?.path ?? null })
                  setTree(next)
                  toast(t('views.folders.removed', { name: node.name }), 3500)
                },
                () => toast(t('err.unknown'))
              )
        }
      ],
      at
    )
  }
  const toggle = (p: string): void =>
    setOpen((o) => {
      const n = new Set(o)
      if (!n.delete(p)) n.add(p)
      return n
    })

  const onKey = (e: KeyboardEvent<HTMLUListElement>): void => {
    const i = rows.findIndex((r) => r.node.path === (focus ?? folder))
    const r = rows[i]
    const to = (j: number): void => {
      const n = rows[Math.max(0, Math.min(rows.length - 1, j))]
      if (n) setFocus(n.node.path)
    }
    if (e.key === 'ArrowDown') to(i + 1)
    else if (e.key === 'ArrowUp') to(i - 1)
    else if (e.key === 'Home') to(0)
    else if (e.key === 'End') to(rows.length - 1)
    else if (e.key === 'ArrowRight' && r) {
      if (r.node.children.length && !open.has(r.node.path)) toggle(r.node.path)
      else if (r.node.children.length) to(i + 1)
    } else if (e.key === 'ArrowLeft' && r) {
      if (open.has(r.node.path)) toggle(r.node.path)
      else if (r.parent) setFocus(r.parent)
    } else if ((e.key === 'Enter' || e.key === ' ') && r) select(r.node.path)
    else if (isMenuKey(e) && r) menu(r.node, e.target as HTMLElement)
    else return
    e.preventDefault()
  }

  // Roving tabindex: keep DOM focus on the focused item while the tree has focus
  useEffect(() => {
    const el = treeRef.current
    if (!el?.contains(document.activeElement) || !focus) return
    el.querySelector<HTMLElement>(`[data-path="${CSS.escape(focus)}"]`)?.focus()
  }, [focus])

  if (tree && !tree.length)
    return (
      <div className="vw vw-folders">
        <ViewHead title={t('nav.folders')} />
        <Empty title={t('library.empty')} hint={t('library.emptyHint')} />
      </div>
    )

  const focused = focus ?? folder
  return (
    <div className="vw vw-folders">
      <ViewHead
        kicker={t('nav.folders')}
        title={<span className="vw-path" title={folder ?? ''}>{name || t('app.loading')}</span>}
        meta={list.ready && <span>{list.total === 1 ? t('library.song') : t('library.songs', { n: list.total })} · {fmtTotal(duration)}</span>}
      >
        <Switch checked={recursive} onChange={setRecursive}>
          {t('views.folders.recursive')}
        </Switch>
        <button className="btn primary" disabled={!list.total} onClick={() => void api.playView(view).catch(() => {})}>
          {t('views.folders.play')}
        </button>
      </ViewHead>
      <div className="vw-split">
        <nav className="vw-pane vw-tree-pane" aria-label={t('views.folders.tree')}>
          <ul ref={treeRef} className="vw-tree" role="tree" aria-label={t('views.folders.tree')} onKeyDown={onKey}>
            {rows.map(({ node, level }) => {
              const kids = node.children.length > 0
              const isOpen = open.has(node.path)
              const on = node.path === folder
              return (
                <li
                  key={node.path}
                  role="treeitem"
                  data-path={node.path}
                  aria-level={level}
                  aria-expanded={kids ? isOpen : undefined}
                  aria-selected={on}
                  tabIndex={node.path === focused ? 0 : -1}
                  className={`vw-node ${on ? 'on' : ''}`}
                  style={{ '--lvl': level - 1 } as CSSProperties}
                  onFocus={() => setFocus(node.path)}
                  onClick={() => select(node.path)}
                  onContextMenu={(e) => {
                    e.preventDefault()
                    menu(node)
                  }}
                  title={node.path}
                >
                  <span
                    className={`vw-twist ${kids ? '' : 'leaf'} ${isOpen ? 'open' : ''}`}
                    aria-hidden="true"
                    onClick={(e) => {
                      if (!kids) return
                      e.stopPropagation()
                      toggle(node.path)
                    }}
                  >
                    <Ico.ChevronRight />
                  </span>
                  <span className="vw-node-n">{node.name}</span>
                  <span className="vw-count" aria-label={t('library.songs', { n: node.totalCount })}>
                    {node.totalCount}
                  </span>
                </li>
              )
            })}
          </ul>
        </nav>
        <div className="vw-pane vw-list-pane">
          {list.ready && list.total === 0 ? (
            <Empty title={t('views.folders.none')} hint={!recursive && selNode && selNode.totalCount > 0 ? t('views.folders.noneHint') : undefined}>
              {!recursive && selNode && selNode.totalCount > 0 && (
                <button className="btn" onClick={() => setRecursive(true)}>
                  {t('views.folders.recursive')}
                </button>
              )}
            </Empty>
          ) : (
            <TrackList
              key={`${folder}|${recursive}`}
              total={list.total}
              rows={list.rows}
              ensureRange={list.ensureRange}
              label={name}
              onPlay={(_, tr) => void api.playView(view, tr.id).catch(() => {})}
            />
          )}
        </div>
      </div>
    </div>
  )
}
