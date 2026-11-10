import { test } from 'node:test'
import assert from 'node:assert/strict'
import es from './es.ts'
import en from './en.ts'

const keys = (o: object, p = ''): string[] =>
  Object.entries(o).flatMap(([k, v]) => (v && typeof v === 'object' && !Array.isArray(v) ? keys(v, `${p}${k}.`) : [`${p}${k}`]))

test('en has every es key', () => {
  const missing = keys(es).filter((k) => !keys(en).includes(k))
  assert.deepEqual(missing, [])
})

test('every literal t() key used in src exists in es', async () => {
  const { readdir, readFile } = await import('node:fs/promises')
  const files = (await readdir('src', { recursive: true })).filter((f) => /\.tsx?$/.test(f) && !f.endsWith('.test.ts'))
  const known = new Set(keys(es))
  const missing: string[] = []
  for (const f of files) {
    const src = await readFile(`src/${f}`, 'utf8')
    for (const m of src.matchAll(/\bt\(\s*'([a-zA-Z0-9_.]+)'/g)) if (!known.has(m[1]) && !m[1].startsWith('err.')) missing.push(`${f}: ${m[1]}`)
  }
  assert.deepEqual(missing, [])
})
