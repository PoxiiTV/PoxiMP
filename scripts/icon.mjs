// Generates .assets/icon.svg: the suite's pixel "P" in PoxiMP orange with the stepped shadow
// and a small gold eighth note at the bottom right.
// Usage: node scripts/icon.mjs  (then: npx tauri icon .assets/icon.svg -o src-tauri/icons)
import { writeFileSync } from 'node:fs'

const B = 4 // pixel size
const P = ['#####.', '##..##', '##..##', '#####.', '##....', '##....', '##....']
const letter = P.flatMap((row, y) => [...row].map((c, x) => (c === '#' ? [x * B, y * B, B, B] : null)).filter(Boolean))
// Eighth note: stem, flag and head (x, y, w, h)
const note = [[25, 17, 2, 11], [27, 18, 2, 2], [29, 20, 2, 3], [21, 25, 4, 1], [20, 26, 6, 3], [21, 29, 4, 1]]

const path = (list, d) => list.map(([x, y, w, h]) => `M${x + d} ${y + d}h${w}v${h}h-${w}z`).join('')
const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-7 -5 40 40" shape-rendering="crispEdges">\
<rect x="-7" y="-5" width="40" height="40" rx="7" fill="#170a12"/>\
<path fill="#5a2812" d="${path(letter, 2)}"/>\
<path fill="#9c461f" d="${path(letter, 1)}"/>\
<path fill="#d9622b" d="${path(letter, 0)}"/>\
<path fill="#4d3f1d" d="${path(note, 2)}"/>\
<path fill="#997e39" d="${path(note, 1)}"/>\
<path fill="#ffd25f" d="${path(note, 0)}"/></svg>`
writeFileSync(new URL('../.assets/icon.svg', import.meta.url), svg)
console.log('ok')
