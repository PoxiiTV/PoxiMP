import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { changesFor, changesSince, isNewer } from './changelog.ts'

const raw = readFileSync(new URL('../../CHANGELOG.md', import.meta.url), 'utf8')

test('version order', () => {
  assert.ok(isNewer('0.10.0', '0.9.2'))
  assert.ok(!isNewer('0.3.0', '0.3.0'))
  assert.ok(!isNewer('0.2.9', '0.3.0'))
})

test('cards come from the CHANGELOG, sub-bullets folded in', () => {
  const es = changesFor(raw, '0.3.0', 'es')
  assert.equal(es[0].emoji, '🎛️')
  assert.equal(es[0].title, 'Efectos')
  assert.match(es[0].text, /Crossfeed para cascos/)
  assert.ok(!es.some((c) => /\*\*|`/.test(c.title + c.text)), 'no Markdown left')
  assert.equal(changesFor(raw, '0.3.0', 'en')[0].title, 'Effects')
  assert.deepEqual(changesFor(raw, '9.9.9', 'es'), [])
  // Since 0.1.0 up to 0.3.0: both newer versions, newest first, nothing from 0.1.0
  const since = changesSince(raw, '0.1.0', '0.3.0', 'es')
  assert.equal(since.length, changesFor(raw, '0.3.0', 'es').length + changesFor(raw, '0.2.0', 'es').length)
  assert.equal(since[0].title, 'Efectos')
})
