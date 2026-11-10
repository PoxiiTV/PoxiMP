// PoxiMP phone remote. No framework, no inline code (the page's CSP only allows its own files).
// Text from the library always goes in with textContent, never as HTML.
'use strict'

const L = {
  es: {
    connecting: 'Conectando con PoxiMP…',
    expired: 'Escanea el código otra vez',
    expiredHint: 'Este enlace ya no vale. En el ordenador, abre PoxiMP → Ajustes → Mando desde el móvil y escanea el código.',
    offline: 'Sin conexión con PoxiMP',
    offlineHint: 'Comprueba que el móvil está en la misma wifi que el ordenador y que el mando sigue encendido. Reintentando…',
    tooMany: 'Demasiados intentos',
    tooManyHint: 'Espera un minuto y escanea el código otra vez.',
    live: 'Conectado',
    reconnecting: 'Reconectando…',
    nothing: 'No suena nada',
    nothingHint: 'Busca una canción para empezar.',
    untitled: 'Sin título',
    play: 'Reproducir',
    pause: 'Pausa',
    prev: 'Anterior',
    next: 'Siguiente',
    shuffle: 'Aleatorio',
    repeat: 'Repetir',
    seek: 'Posición',
    volume: 'Volumen',
    locked: 'Bit-perfect activo: el volumen se ajusta en el amplificador o el DAC.',
    search: 'Buscar en tu biblioteca',
    upNext: 'A continuación',
    queueEmpty: 'La cola está vacía.',
    noResults: 'Nada con ese nombre.',
    tabNow: 'Suena',
    tabQueue: 'Cola',
    tabSearch: 'Buscar',
    clear: 'Borrar',
    searchHint: 'Escribe para buscar en tu biblioteca.',
    q: { real: 'Real', noSigns: 'Sin indicios', suspicious: 'Dudoso', fake: 'Falso', lossyOrigin: 'Origen con pérdida', fakeHiRes: 'Hi-Res falso', inconclusive: 'Sin veredicto', analyzing: 'Analizando…', pending: 'Sin analizar' }
  },
  en: {
    connecting: 'Connecting to PoxiMP…',
    expired: 'Scan the code again',
    expiredHint: 'This link no longer works. On the computer, open PoxiMP → Settings → Phone remote and scan the code.',
    offline: 'No connection to PoxiMP',
    offlineHint: 'Check that the phone is on the same Wi-Fi as the computer and that the remote is still on. Retrying…',
    tooMany: 'Too many tries',
    tooManyHint: 'Wait a minute and scan the code again.',
    live: 'Connected',
    reconnecting: 'Reconnecting…',
    nothing: 'Nothing playing',
    nothingHint: 'Search for a song to start.',
    untitled: 'Untitled',
    play: 'Play',
    pause: 'Pause',
    prev: 'Previous',
    next: 'Next',
    shuffle: 'Shuffle',
    repeat: 'Repeat',
    seek: 'Position',
    volume: 'Volume',
    locked: 'Bit-perfect is on: set the volume on the amplifier or the DAC.',
    search: 'Search your library',
    upNext: 'Up next',
    queueEmpty: 'The queue is empty.',
    noResults: 'Nothing by that name.',
    tabNow: 'Playing',
    tabQueue: 'Queue',
    tabSearch: 'Search',
    clear: 'Clear',
    searchHint: 'Type to search your library.',
    q: { real: 'Real', noSigns: 'No signs', suspicious: 'Doubtful', fake: 'Fake', lossyOrigin: 'Lossy source', fakeHiRes: 'Fake Hi-Res', inconclusive: 'No verdict', analyzing: 'Analysing…', pending: 'Not analysed' }
  }
}
const lang = (navigator.language || 'en').toLowerCase().startsWith('es') ? 'es' : 'en'
const T = L[lang]
const $ = (id) => document.getElementById(id)
const BADGE = { real: 'ok', noSigns: 'clean', suspicious: 'warn', fake: 'bad', lossyOrigin: 'bad', fakeHiRes: 'bad' }
const REPEAT_NEXT = { off: 'all', all: 'one', one: 'off' }

document.documentElement.lang = lang
for (const el of document.querySelectorAll('[data-t]')) el.textContent = T[el.dataset.t]
for (const el of document.querySelectorAll('[data-ta]')) el.setAttribute('aria-label', T[el.dataset.ta])
for (const el of document.querySelectorAll('[data-tp]')) el.placeholder = T[el.dataset.tp]

let st = null // last state from the computer
let at = 0 // when it arrived (performance.now)
let es = null
let retry = 0
let expired = false
let seeking = false
let volDrag = false
let volTimer = 0
let queueKey = ''
let searchSeq = 0
let searchTimer = 0
let tab = 'now'

class Expired extends Error {}

/** GET (no body) or POST JSON; 401 = the session is gone. */
async function req(path, body) {
  const init = body === undefined
    ? { cache: 'no-store' }
    : { method: 'POST', cache: 'no-store', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }
  const r = await fetch(path, init)
  if (r.status === 401) throw new Expired()
  if (!r.ok) throw Object.assign(new Error('http'), { status: r.status })
  return r.json()
}

const cmd = (name, args) => req('/api/cmd', { cmd: name, ...args }).catch((e) => {
  if (e instanceof Expired) show('expired')
})

function time(secs) {
  const s = Math.max(0, Math.floor(secs || 0))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const ss = String(s % 60).padStart(2, '0')
  return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`
}

const khz = (hz) => (hz / 1000).toLocaleString(lang === 'es' ? 'es-ES' : 'en-GB', { maximumFractionDigits: 1, useGrouping: false })
const formatLabel = (t) => (t.lossless && t.bitDepth ? `${t.format} ${t.bitDepth}/${khz(t.sampleRate)}` : `${t.format} ${Math.round(t.bitrate)}`)

// --- connection ---

/** Full-screen message: connecting, expired, offline (before the first state), tooMany. */
function show(kind) {
  if (kind === 'expired') {
    expired = true
    clearTimeout(retry)
    if (es) es.close()
    es = null
  }
  $('gate').dataset.kind = kind
  $('gate-title').textContent = T[kind]
  $('gate-text').textContent = T[kind + 'Hint'] || ''
  $('gate').hidden = false
  $('app').hidden = true
  $('dock').hidden = true
}

function online(ok) {
  if (!ok && !st) return show('offline')
  if (ok) {
    $('gate').hidden = true
    $('app').hidden = false
    $('dock').hidden = false
  }
  $('app').classList.toggle('off', !ok)
  $('conn').classList.toggle('off', !ok)
  $('conn').textContent = ok ? T.live : T.reconnecting
}

function connect() {
  if (es || expired) return
  es = new EventSource('/api/events')
  es.addEventListener('state', (e) => {
    online(true)
    render(JSON.parse(e.data))
  })
  es.onerror = () => {
    if (es) es.close()
    es = null
    online(false)
    clearTimeout(retry)
    retry = setTimeout(check, 2000)
  }
}

/** After a drop: still paired (reconnect), signed out (expired) or unreachable (try again). */
async function check() {
  clearTimeout(retry)
  if (es || expired) return
  try {
    await req('/api/state')
    connect()
  } catch (e) {
    if (e instanceof Expired) show('expired')
    else retry = setTimeout(check, 3000)
  }
}

async function boot() {
  const token = location.hash.slice(1)
  if (token) {
    // The code never stays in the address bar or the history.
    history.replaceState(null, '', location.pathname)
    try {
      await req('/pair', { token })
    } catch (e) {
      return show(e instanceof Expired ? 'expired' : e.status === 429 ? 'tooMany' : 'offline')
    }
  }
  connect()
}

document.addEventListener('visibilitychange', () => {
  if (!document.hidden && !es) check()
})

// --- now playing ---

function setCover(t) {
  const img = $('cover')
  const bg = $('bg')
  img.hidden = true
  bg.classList.remove('on')
  $('np-cover').hidden = true
  if (!t) {
    $('np-cover').removeAttribute('src')
    return img.removeAttribute('src')
  }
  const src = `/api/cover/${t.id}`
  img.onload = () => {
    img.hidden = false
    bg.style.backgroundImage = `url("${src}")`
    bg.classList.add('on')
  }
  img.onerror = () => img.removeAttribute('src')
  img.src = src
  const mini = $('np-cover')
  mini.hidden = true
  mini.onload = () => { mini.hidden = false }
  mini.onerror = () => mini.removeAttribute('src')
  mini.src = src
}

function fill(input) {
  const max = Number(input.max) || 1
  input.style.setProperty('--p', `${(Number(input.value) / max) * 100}%`)
}

/** Seconds now, from the last state and the clock (the computer only sends news). */
function nowSecs() {
  if (!st) return 0
  const run = st.status === 'playing' ? ((performance.now() - at) / 1000) * (st.speed || 1) : 0
  return st.duration > 0 ? Math.min(st.secs + run, st.duration) : st.secs + run
}

function tick() {
  if (!st || seeking) return
  const s = nowSecs()
  const seek = $('seek')
  seek.value = st.duration > 0 ? Math.round((s / st.duration) * 1000) : 0
  fill(seek)
  $('cur').textContent = time(s)
  $('dur').textContent = time(st.duration)
}

function render(s) {
  const prev = st
  st = s
  at = performance.now()
  const t = s.track
  $('title').textContent = t ? t.title || T.untitled : T.nothing
  $('artist').textContent = t ? [t.artist, t.album].filter(Boolean).join(' · ') : T.nothingHint
  if (!prev || prev.track?.id !== t?.id) setCover(t)
  const badge = $('badge')
  badge.hidden = !t
  if (t) {
    badge.className = `badge ${BADGE[t.quality] || 'none'}`
    badge.textContent = T.q[t.quality] || T.q.pending
  }
  $('fmt').textContent = t ? formatLabel(t) : ''
  const playing = s.status === 'playing'
  for (const b of [$('play'), $('np-play')]) {
    b.classList.toggle('on', playing)
    b.setAttribute('aria-label', playing ? T.pause : T.play)
  }
  $('np-title').textContent = t ? t.title || T.untitled : T.nothing
  $('np-artist').textContent = t ? t.artist : ''
  $('np').hidden = !t || tab === 'now'
  $('shuffle').setAttribute('aria-pressed', String(s.shuffle))
  $('repeat').setAttribute('aria-pressed', String(s.repeat !== 'off'))
  $('repeat').classList.toggle('one', s.repeat === 'one')
  const vol = $('vol')
  if (!volDrag) {
    vol.value = s.volume
    fill(vol)
  }
  vol.disabled = s.volumeLocked
  $('locked').hidden = !s.volumeLocked
  $('seek').disabled = !t
  const key = `${s.queueRev}:${s.position}:${s.queueLength}`
  if (key !== queueKey) {
    queueKey = key
    loadQueue()
  }
  tick()
}

setInterval(tick, 250)

$('play').addEventListener('click', () => cmd('toggle'))
$('np-play').addEventListener('click', () => cmd('toggle'))
$('prev').addEventListener('click', () => cmd('prev'))
$('next').addEventListener('click', () => cmd('next'))
$('np-next').addEventListener('click', () => cmd('next'))
$('shuffle').addEventListener('click', () => st && cmd('shuffle', { on: !st.shuffle }))
$('repeat').addEventListener('click', () => st && cmd('repeat', { mode: REPEAT_NEXT[st.repeat] || 'off' }))

const seekSecs = () => (Number($('seek').value) / 1000) * (st ? st.duration : 0)
$('seek').addEventListener('input', () => {
  seeking = true
  fill($('seek'))
  $('cur').textContent = time(seekSecs())
})
$('seek').addEventListener('change', () => {
  const secs = seekSecs()
  seeking = false
  if (!st) return
  st.secs = secs
  at = performance.now()
  cmd('seek', { secs })
})

const sendVolume = () => cmd('volume', { value: Number($('vol').value) })
// The capsule has no thumb: a touch anywhere on it sets the level, and dragging follows the finger.
function volAt(e) {
  const vol = $('vol')
  const r = vol.getBoundingClientRect()
  vol.value = Math.round(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * 100)
  vol.dispatchEvent(new Event('input'))
}
$('vol').addEventListener('pointerdown', (e) => {
  if ($('vol').disabled) return
  e.preventDefault()
  $('vol').setPointerCapture(e.pointerId)
  volAt(e)
})
$('vol').addEventListener('pointermove', (e) => {
  if ($('vol').hasPointerCapture(e.pointerId)) volAt(e)
})
$('vol').addEventListener('pointerup', (e) => {
  if (!$('vol').hasPointerCapture(e.pointerId)) return
  $('vol').releasePointerCapture(e.pointerId)
  $('vol').dispatchEvent(new Event('change'))
})
$('vol').addEventListener('input', () => {
  volDrag = true
  fill($('vol'))
  // While dragging: a few updates a second, not one per pixel.
  if (!volTimer) {
    volTimer = setTimeout(() => {
      volTimer = 0
      sendVolume()
    }, 150)
  }
})
$('vol').addEventListener('change', () => {
  volDrag = false
  clearTimeout(volTimer)
  volTimer = 0
  sendVolume()
})

// --- queue and search ---

function list(where, tracks, emptyText, onPick, current) {
  const empty = $(where + 'empty')
  empty.hidden = tracks.length > 0
  empty.textContent = emptyText
  const items = tracks.map((t, i) => {
    const li = document.createElement('li')
    const row = document.createElement('button')
    row.type = 'button'
    row.className = i === current ? 'row cur' : 'row'
    const thumb = document.createElement('span')
    thumb.className = 'thumb'
    const img = document.createElement('img')
    img.alt = ''
    img.loading = 'lazy'
    img.decoding = 'async'
    img.onerror = () => img.remove()
    img.src = `/api/cover/${t.id}`
    thumb.append(img)
    const text = document.createElement('span')
    const name = document.createElement('strong')
    name.textContent = t.title || T.untitled
    const sub = document.createElement('small')
    sub.textContent = t.artist
    text.append(name, sub)
    const d = document.createElement('span')
    d.className = 'd'
    d.textContent = time(t.duration)
    row.append(thumb, text, d)
    row.addEventListener('click', () => onPick(t, i))
    li.append(row)
    return li
  })
  $(where + 'items').replaceChildren(...items)
}

async function loadQueue() {
  const from = Math.max(0, st ? st.position : 0)
  try {
    const q = await req(`/api/queue?offset=${from}`)
    list('q', q.tracks, T.queueEmpty, (_, i) => cmd('jump', { position: q.offset + i }), q.position - q.offset)
  } catch (e) {
    if (e instanceof Expired) show('expired')
  }
}

async function search() {
  const text = $('q').value.trim()
  const mine = ++searchSeq
  $('qx').hidden = !text
  if (!text) return list('s', [], T.searchHint, () => {}, -1)
  try {
    const r = await req('/api/cmd', { cmd: 'search', q: text })
    if (mine !== searchSeq) return
    list('s', r.tracks, T.noResults, (t) => cmd('playTrack', { id: t.id }), -1)
  } catch (e) {
    if (e instanceof Expired) show('expired')
  }
}

$('q').addEventListener('input', () => {
  clearTimeout(searchTimer)
  searchTimer = setTimeout(search, 250)
})
$('qx').addEventListener('click', () => {
  $('q').value = ''
  search()
  $('q').focus()
})

// --- tabs: Playing · Queue · Search (docs/bocetos/12-mando-movil.html, design B) ---

function go(name) {
  tab = name
  $('app').dataset.tab = name
  for (const v of ['now', 'queue', 'search']) $('v-' + v).hidden = v !== name
  for (const b of document.querySelectorAll('.tabs [data-tab]')) b.setAttribute('aria-selected', String(b.dataset.tab === name))
  $('np').hidden = !st?.track || name === 'now'
  window.scrollTo(0, 0)
  // Same tap that opened the tab, so iOS lets the keyboard come up.
  if (name === 'search' && !$('q').value) $('q').focus()
}
for (const b of document.querySelectorAll('.tabs [data-tab]')) b.addEventListener('click', () => go(b.dataset.tab))
$('np-go').addEventListener('click', () => go('now'))
list('s', [], T.searchHint, () => {}, -1)

show('connecting')
boot()
