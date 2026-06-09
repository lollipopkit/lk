import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { join } from 'node:path'

const prebuiltFiles = [
  'src/wasm/pkg/lk_wasm.js',
  'src/wasm/pkg/lk_wasm.d.ts',
  'src/wasm/pkg/lk_wasm_bg.wasm',
  'src/wasm/pkg/lk_wasm_bg.wasm.d.ts',
  'src/wasm/pkg/package.json',
]

const hasPrebuiltWasm = prebuiltFiles.every((file) => existsSync(join(process.cwd(), file)))
const isCloudflarePages = process.env.CF_PAGES === '1' || process.env.CF_PAGES === 'true'

if (isCloudflarePages && hasPrebuiltWasm) {
  console.log('Using checked-in LK wasm package for Cloudflare Pages build.')
  process.exit(0)
}

const result = spawnSync(
  'wasm-pack',
  ['build', '../wasm', '--target', 'web', '--out-dir', '../website/src/wasm/pkg', '--release'],
  { stdio: 'inherit' },
)

if (result.error) {
  console.error(result.error.message)
  process.exit(1)
}

if (result.status === 0) {
  const jsPath = join(process.cwd(), 'src/wasm/pkg/lk_wasm.js')
  const js = readFileSync(jsPath, 'utf8')

  if (!js.startsWith('// @ts-nocheck')) {
    writeFileSync(jsPath, `// @ts-nocheck\n${js}`)
  }
}

process.exit(result.status ?? 1)
