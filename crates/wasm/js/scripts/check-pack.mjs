// Packs the package, installs the tarball in a new Vite project, and checks
// that tsc passes and the build emits the wasm and the four workers as files.
// Run `npm run wasm` and `npm run build` first.
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const packageDir = resolve(import.meta.dirname, '..')
const workspaceModules = resolve(packageDir, '../../../node_modules')
const versionOf = (name) => JSON.parse(readFileSync(join(workspaceModules, name, 'package.json'), 'utf8')).version
const run = (command, args, cwd) => execFileSync(command, args, { cwd, stdio: 'inherit' })

const projectDir = mkdtempSync(join(tmpdir(), 'moq-wasm-consumer-'))
const [{ filename }] = JSON.parse(
  execFileSync('npm', ['pack', '--json', '--pack-destination', projectDir], { cwd: packageDir, encoding: 'utf8' })
)
writeFileSync(
  join(projectDir, 'package.json'),
  JSON.stringify({
    name: 'moq-wasm-consumer',
    private: true,
    type: 'module',
    dependencies: { 'moq-wasm': `file:./${filename}` },
    devDependencies: {
      '@types/dom-mediacapture-transform': versionOf('@types/dom-mediacapture-transform'),
      typescript: versionOf('typescript'),
      vite: versionOf('vite')
    }
  })
)
writeFileSync(
  join(projectDir, 'tsconfig.json'),
  JSON.stringify({
    compilerOptions: {
      target: 'esnext',
      module: 'esnext',
      moduleResolution: 'bundler',
      strict: true,
      noEmit: true,
      types: ['@types/dom-mediacapture-transform', 'vite/client'],
      lib: ['esnext', 'dom']
    },
    include: ['main.ts']
  })
)
writeFileSync(join(projectDir, 'index.html'), '<script type="module" src="./main.ts"></script>\n')
writeFileSync(
  join(projectDir, 'main.ts'),
  `import { MoqSession } from 'moq-wasm'
import { LivePlayer, publish } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

const session: MoqSession = await MoqSession.connect('https://relay.example:4433')
await publish(session, ['demo'], new MediaStream())
console.log(LivePlayer)
`
)
run('npm', ['install', '--no-audit', '--no-fund'], projectDir)
run('npx', ['tsc'], projectDir)
run('npx', ['vite', 'build'], projectDir)

const assets = readdirSync(join(projectDir, 'dist', 'assets'))
const missing = ['.wasm', 'videoEncoder', 'audioEncoder', 'videoDecoder', 'audioDecoder'].filter(
  (part) => !assets.some((asset) => asset.includes(part))
)
if (missing.length > 0) {
  console.error(`The consumer build lacks ${missing.join(', ')}; it has ${assets.join(', ')}`)
  process.exit(1)
}
console.log(`The consumer build in ${projectDir} has the wasm and the workers.`)
