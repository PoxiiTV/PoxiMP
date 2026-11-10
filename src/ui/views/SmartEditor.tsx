// Smart playlist editor: nested Todas/Cualquiera groups, rows adapted to the field type, limit + sort,
// and a live preview (smart_preview, debounced).
import { useEffect, useId, useMemo, useState, type ReactNode } from 'react'
import { api } from '../../core/api'
import { errorText, useT } from '../../core/i18n'
import type { PlaylistPage, SmartField, SmartGroup, SmartNode, SmartOp, SmartRule, SmartRules, SmartSort, Verdict } from '../../core/types'
import * as Ico from '../shell/icons'
import { toast } from '../TrackMenu'
import { TrackList, ViewHead } from './common'
import type { Editing } from './Playlists'

const DAY = 86400
const MAX_DEPTH = 8

type Kind = 'text' | 'verdict' | 'number' | 'date'
const KIND: Record<SmartField, Kind> = {
  title: 'text', artist: 'text', album: 'text', album_artist: 'text', genre: 'text', codec: 'text', folder: 'text',
  quality_verdict: 'verdict',
  year: 'number', duration: 'number', bitrate: 'number', sample_rate: 'number', bit_depth: 'number', play_count: 'number', skip_count: 'number', rating: 'number',
  last_played_at: 'date', added_at: 'date'
}
const GROUPS: [string, SmartField[]][] = [
  ['song', ['title', 'artist', 'album', 'album_artist', 'genre', 'year', 'duration']],
  ['file', ['codec', 'bitrate', 'sample_rate', 'bit_depth', 'quality_verdict', 'folder']],
  ['listening', ['play_count', 'skip_count', 'rating', 'last_played_at', 'added_at']]
]
const OPS: Record<Kind, SmartOp[]> = {
  text: ['contains', 'not_contains', 'is', 'is_not', 'starts_with', 'is_empty'],
  verdict: ['is', 'is_not', 'is_empty'],
  number: ['is', 'is_not', 'gt', 'lt', 'between', 'is_empty'],
  date: ['in_last_days', 'not_in_last_days', 'gt', 'lt', 'between', 'is_empty']
}
const VERDICTS: Verdict[] = ['real', 'noSigns', 'suspicious', 'fake', 'lossyOrigin', 'fakeHiRes', 'inconclusive']
const SORTS: SmartSort[] = ['artist', 'title', 'album', 'year', 'most_played', 'least_played', 'recently_played', 'recently_added', 'most_skipped', 'rating', 'random']
/** Shown in minutes, stored in seconds */
const SCALE: Partial<Record<SmartField, number>> = { duration: 60 }

export const NEW_RULES: SmartRules = { all: [{ field: 'play_count', op: 'gt', value: 5 }], sort: 'most_played' }

const now = (): number => Math.floor(Date.now() / 1000)
const toDate = (secs: number): string => {
  const d = new Date(secs * 1000)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}
const fromDate = (s: string): number => {
  const [y, m, d] = s.split('-').map(Number)
  return y ? Math.floor(new Date(y, m - 1, d).getTime() / 1000) : now()
}

/** A sensible value when the field or the operator changes. */
function defaultValue(field: SmartField, op: SmartOp): SmartRule['value'] {
  const k = KIND[field]
  if (op === 'is_empty') return null
  if (op === 'in_last_days' || op === 'not_in_last_days') return 30
  if (k === 'date') return op === 'between' ? [now() - 60 * DAY, now()] : now() - 30 * DAY
  if (k === 'verdict') return 'real'
  if (k === 'text') return ''
  if (op === 'between') return field === 'year' ? [2010, 2019] : [0, 10]
  return field === 'year' ? 2020 : field === 'rating' ? 4 : field === 'duration' ? 600 : 0
}

const isGroup = (n: SmartNode): n is SmartGroup => 'all' in n || 'any' in n
const children = (g: SmartGroup): SmartNode[] => ('all' in g ? g.all : g.any)
const withChildren = (g: SmartGroup, c: SmartNode[]): SmartGroup => ('all' in g ? { all: c } : { any: c })

export function SmartEditor({ editing, onClose, onSaved }: { editing: Editing; onClose: () => void; onSaved: (id: number) => void }) {
  const t = useT()
  const nameId = useId()
  const [name, setName] = useState(editing.name)
  const [root, setRoot] = useState<SmartGroup>(() => ('all' in editing.rules ? { all: editing.rules.all } : { any: editing.rules.any }))
  const [limit, setLimit] = useState<number | null>(editing.rules.limit ?? null)
  const [sort, setSort] = useState<SmartSort>(editing.rules.sort ?? 'artist')
  const [preview, setPreview] = useState<{ page: PlaylistPage | null; error: string | null; busy: boolean }>({ page: null, error: null, busy: true })
  const [saving, setSaving] = useState(false)

  const rules: SmartRules = useMemo(() => ({ ...root, sort, ...(limit ? { limit } : {}) }), [root, sort, limit])

  useEffect(() => {
    let alive = true
    setPreview((p) => ({ ...p, busy: true }))
    const id = setTimeout(() => {
      api.smartPreview(rules, 50).then(
        (page) => alive && setPreview({ page, error: null, busy: false }),
        (e) => alive && setPreview({ page: null, error: errorText(e), busy: false })
      )
    }, 320)
    return () => {
      alive = false
      clearTimeout(id)
    }
  }, [rules])

  const save = (): void => {
    const n = name.trim()
    if (!n) return void document.getElementById(nameId)?.focus()
    setSaving(true)
    const p =
      editing.id === null
        ? api.playlistCreate(n.slice(0, 200), rules).then((pl) => pl.id)
        : api
            .playlistSetRules(editing.id, rules)
            .then(() => (n !== editing.name ? api.playlistRename(editing.id!, n.slice(0, 200)) : undefined))
            .then(() => editing.id!)
    p.then(onSaved, (e) => {
      setSaving(false)
      toast(errorText(e))
    })
  }

  const pg = preview.page
  return (
    <div className="vw vw-smart">
      <ViewHead kicker={editing.id === null ? t('nav.playlists') : t('views.smart.editKicker')} title={editing.id === null ? t('views.playlists.newSmart') : editing.name}>
        <button className="btn" onClick={onClose}>
          {t('eq.cancel')}
        </button>
        <button className="btn primary" disabled={saving || !name.trim()} onClick={save}>
          {t('eq.save')}
        </button>
      </ViewHead>
      <div className="vw-smart-body">
        <div className="vw-pane vw-rules">
          <label className="vw-field vw-name" htmlFor={nameId}>
            <span>{t('views.playlists.name')}</span>
            <input id={nameId} className="vw-input" value={name} maxLength={200} placeholder={t('views.smart.namePlaceholder')} onChange={(e) => setName(e.target.value)} />
          </label>
          <Group group={root} depth={1} onChange={setRoot} />
          <div className="vw-limits">
            <label className="vw-check">
              <input type="checkbox" checked={limit !== null} onChange={(e) => setLimit(e.target.checked ? 25 : null)} />
              <span>{t('views.smart.limit')}</span>
            </label>
            <NumInput value={limit ?? 25} min={1} max={10000} disabled={limit === null} label={t('views.smart.limitN')} onChange={(v) => setLimit(Math.max(1, Math.min(10000, Math.round(v))))} />
            <span className="vw-unit">{t('views.smart.songs')}</span>
            <label className="vw-field vw-sort">
              <span>{t('views.smart.sort')}</span>
              <select className="vw-select" value={sort} onChange={(e) => setSort(e.target.value as SmartSort)}>
                {SORTS.map((s) => (
                  <option key={s} value={s}>
                    {t(`views.smart.sorts.${s}`)}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>
        <section className="vw-pane vw-preview" aria-label={t('views.smart.preview')} aria-busy={preview.busy}>
          <header className="vw-preview-h">
            <h3 className="vw-sec-t">{t('views.smart.preview')}</h3>
            <span className={`vw-meta ${preview.busy ? 'busy' : ''}`} aria-live="polite">
              {preview.error ? '' : pg ? (pg.total === 1 ? t('views.smart.matchOne') : t('views.smart.matches', { n: pg.total })) : ''}
            </span>
          </header>
          {preview.error ? (
            <p className="vw-hint warn">{preview.error}</p>
          ) : pg && pg.total === 0 ? (
            <p className="vw-hint">{t('views.smart.noMatches')}</p>
          ) : (
            pg && <TrackList total={pg.tracks.length} rows={pg.tracks} ensureRange={noop} label={t('views.smart.preview')} album={false} onPlay={(_, tr) => void api.playTrack(tr.id).catch(() => {})} className="compact" />
          )}
          {pg && pg.total > pg.tracks.length && <p className="vw-foot">{t('views.smart.more', { n: pg.total - pg.tracks.length })}</p>}
        </section>
      </div>
    </div>
  )
}

const noop = (): void => {}

function Group({ group, depth, onChange, onRemove }: { group: SmartGroup; depth: number; onChange: (g: SmartGroup) => void; onRemove?: () => void }) {
  const t = useT()
  const list = children(group)
  const mode = 'all' in group ? 'all' : 'any'
  const set = (c: SmartNode[]): void => onChange(withChildren(group, c))
  const at = (i: number) => (n: SmartNode) => set(list.map((x, j) => (j === i ? n : x)))
  const drop = (i: number) => () => set(list.filter((_, j) => j !== i))
  return (
    <div className={`vw-group d${Math.min(depth, 4)}`} role="group" aria-label={t(`views.smart.mode.${mode}`)}>
      <div className="vw-group-h">
        <div className="seg" role="radiogroup" aria-label={t('views.smart.match')}>
          {(['all', 'any'] as const).map((m) => (
            <button key={m} role="radio" aria-checked={mode === m} className={mode === m ? 'on' : ''} onClick={() => onChange(m === 'all' ? { all: list } : { any: list })}>
              {t(`views.smart.mode.${m}`)}
            </button>
          ))}
        </div>
        <span className="vw-group-l">{t('views.smart.ofThese')}</span>
        {onRemove && (
          <button className="vw-x" aria-label={t('views.smart.removeGroup')} title={t('views.smart.removeGroup')} onClick={onRemove}>
            <Ico.Close />
          </button>
        )}
      </div>
      <div className="vw-group-b">
        {list.length === 0 && <p className="vw-hint">{mode === 'all' ? t('views.smart.emptyAll') : t('views.smart.emptyAny')}</p>}
        {list.map((n, i) =>
          isGroup(n) ? (
            <Group key={i} group={n} depth={depth + 1} onChange={at(i)} onRemove={drop(i)} />
          ) : (
            <Rule key={i} rule={n} onChange={at(i)} onRemove={drop(i)} />
          )
        )}
      </div>
      <div className="vw-group-f">
        <button className="btn sm" onClick={() => set([...list, { field: 'artist', op: 'contains', value: '' }])}>
          {t('views.smart.addRule')}
        </button>
        {depth < MAX_DEPTH && (
          <button className="btn sm" onClick={() => set([...list, { any: [{ field: 'genre', op: 'is', value: '' }] }])}>
            {t('views.smart.addGroup')}
          </button>
        )}
      </div>
    </div>
  )
}

function Rule({ rule, onChange, onRemove }: { rule: SmartRule; onChange: (r: SmartRule) => void; onRemove: () => void }) {
  const t = useT()
  const kind = KIND[rule.field]
  const opLabel = (op: SmartOp): string => t(kind === 'date' && ['gt', 'lt', 'between', 'is_empty'].includes(op) ? `views.smart.dateOp.${op}` : `views.smart.op.${op}`)
  const setField = (field: SmartField): void => {
    const op = OPS[KIND[field]].includes(rule.op) && KIND[field] === kind ? rule.op : OPS[KIND[field]][0]
    onChange({ field, op, value: KIND[field] === kind && op === rule.op ? rule.value : defaultValue(field, op) })
  }
  const setOp = (op: SmartOp): void => onChange({ ...rule, op, value: defaultValue(rule.field, op) })
  const set = (value: SmartRule['value']): void => onChange({ ...rule, value })

  return (
    <div className="vw-rule">
      <select className="vw-select vw-f" value={rule.field} aria-label={t('views.smart.field')} onChange={(e) => setField(e.target.value as SmartField)}>
        {GROUPS.map(([g, fields]) => (
          <optgroup key={g} label={t(`views.smart.fieldGroup.${g}`)}>
            {fields.map((f) => (
              <option key={f} value={f}>
                {t(`views.smart.fields.${f}`)}
              </option>
            ))}
          </optgroup>
        ))}
      </select>
      <select className="vw-select vw-o" value={rule.op} aria-label={t('views.smart.op.label')} onChange={(e) => setOp(e.target.value as SmartOp)}>
        {OPS[kind].map((op) => (
          <option key={op} value={op}>
            {opLabel(op)}
          </option>
        ))}
      </select>
      <span className="vw-rule-v">
        <Value rule={rule} kind={kind} set={set} />
      </span>
      <button className="vw-x" aria-label={t('views.smart.removeRule')} title={t('views.smart.removeRule')} onClick={onRemove}>
        <Ico.Close />
      </button>
    </div>
  )
}

function Value({ rule, kind, set }: { rule: SmartRule; kind: Kind; set: (v: SmartRule['value']) => void }) {
  const t = useT()
  const { op, field, value } = rule
  const label = t('views.smart.value')
  if (op === 'is_empty') return null
  if (op === 'in_last_days' || op === 'not_in_last_days')
    return (
      <>
        <NumInput value={Number(value) || 0} min={1} label={label} onChange={(v) => set(Math.max(1, Math.round(v)))} />
        <span className="vw-unit">{t('views.smart.days')}</span>
      </>
    )
  if (kind === 'verdict')
    return (
      <select className="vw-select vw-v" value={String(value ?? 'real')} aria-label={label} onChange={(e) => set(e.target.value)}>
        {VERDICTS.map((v) => (
          <option key={v} value={v}>
            {t(`quality.${v}`, { origin: t('quality.origin.lossy') })}
          </option>
        ))}
      </select>
    )
  if (kind === 'text')
    return <input className="vw-input vw-v" value={String(value ?? '')} aria-label={label} placeholder={t('views.smart.textPlaceholder')} onChange={(e) => set(e.target.value)} spellCheck={false} />
  if (kind === 'date') {
    const one = (v: number, f: (n: number) => void, l = label): ReactNode => <input type="date" className="vw-input vw-v" value={toDate(v)} aria-label={l} onChange={(e) => f(fromDate(e.target.value))} />
    if (op === 'between') {
      const [a, b] = Array.isArray(value) ? value : [now() - 60 * DAY, now()]
      return (
        <>
          {one(a, (x) => set([x, b]), t('views.smart.from'))}
          <span className="vw-unit">{t('views.smart.and')}</span>
          {one(b, (x) => set([a, x]), t('views.smart.to'))}
        </>
      )
    }
    return one(Number(value) || now(), set)
  }
  // number
  const k = SCALE[field] ?? 1
  const unit = t(`views.smart.unit.${field}`)
  const unitEl = unit && <span className="vw-unit">{unit}</span>
  const max = field === 'rating' ? 5 : undefined
  if (op === 'between') {
    const [a, b] = Array.isArray(value) ? value : [0, 10]
    return (
      <>
        <NumInput value={a / k} max={max} label={t('views.smart.from')} onChange={(x) => set([Math.round(x * k), b])} />
        <span className="vw-unit">{t('views.smart.and')}</span>
        <NumInput value={b / k} max={max} label={t('views.smart.to')} onChange={(x) => set([a, Math.round(x * k)])} />
        {unitEl}
      </>
    )
  }
  return (
    <>
      <NumInput value={Number(value) / k} max={max} label={label} onChange={(x) => set(Math.round(x * k))} />
      {unitEl}
    </>
  )
}

/** Number field that lets the text be empty or half-typed while editing. */
function NumInput({ value, onChange, label, min = 0, max, disabled }: { value: number; onChange: (v: number) => void; label: string; min?: number; max?: number; disabled?: boolean }) {
  const [text, setText] = useState(String(value))
  useEffect(() => {
    setText((s) => (Number(s) === value && s !== '' ? s : String(value)))
  }, [value])
  return (
    <input
      type="number"
      inputMode="decimal"
      className="vw-input num"
      value={text}
      min={min}
      max={max}
      disabled={disabled}
      aria-label={label}
      onChange={(e) => {
        setText(e.target.value)
        const n = Number(e.target.value)
        if (e.target.value !== '' && Number.isFinite(n)) onChange(Math.max(min, max === undefined ? n : Math.min(max, n)))
      }}
      onBlur={() => setText(String(value))}
    />
  )
}
