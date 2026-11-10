// Writes deploy-hosting/latest.json, the file the app's updater reads from the GitHub release:
// version, notes (CHANGELOG.md, Spanish part), date and the signed update of this version for the
// platform it runs on (Windows: the installer; macOS: the .app.tar.gz). If deploy-hosting already
// holds a latest.json of the same version (build-mac.sh downloads the release's one first), the
// other platforms in it are kept, so Windows and Mac share one file.
import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join } from 'node:path'

const root = join(import.meta.dirname, '..')
const { version } = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'))
const out = join(root, 'deploy-hosting')
const mac = process.platform === 'darwin'
const platform = mac ? 'darwin-aarch64' : 'windows-x86_64'
const file = mac ? `PoxiMP-${version}-mac-arm64.app.tar.gz` : `PoxiMP-Setup-${version}.exe`
const sig = join(out, `${file}.sig`)
if (!existsSync(sig)) {
  console.error(`  [X] Falta ${file}.sig: el build no se firmó (¿está la clave?). Sin ella no hay actualización automática.`)
  process.exit(1)
}
const log = readFileSync(join(root, 'CHANGELOG.md'), 'utf8').replace(/\r\n/g, '\n')
const start = log.indexOf(`## ${version}\n`)
const es = start < 0 ? '' : log.slice(start).split('\n### ')[1] ?? ''
const notes = es.split('\n').slice(1).join('\n').split('\n## ')[0].trim()
const path = join(out, 'latest.json')
const prev = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : null
const latest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    ...(prev?.version === version ? prev.platforms : {}),
    [platform]: {
      signature: readFileSync(sig, 'utf8').trim(),
      url: `https://github.com/PoxiiTV/PoxiMP/releases/download/v${version}/${file}`
    }
  }
}
writeFileSync(path, JSON.stringify(latest, null, 2) + '\n')
console.log(`  latest.json para ${version}: ${Object.keys(latest.platforms).join(' + ')}`)
