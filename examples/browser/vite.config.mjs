import { defaultClientConditions, defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { globSync } from 'node:fs'
import { resolve } from 'node:path'

// Mirrors the mode utils/access.ts resolves, so the first paint already has the mode's colors: a module
// script runs only after it. utils/access.ts corrects it once resolved.
const ACCESS_MODE_BEFORE_PAINT = `{
  const hasToken = new URLSearchParams(location.search).has('jwt') || new URLSearchParams(location.hash.slice(1)).has('jwt')
  const stored = localStorage.getItem('moqt.accessMode')
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname)
  document.documentElement.dataset.accessMode =
    hasToken ? 'temporary' : stored === 'temporary' || stored === 'public' ? stored : local ? 'public' : 'temporary'
}`

const accessModeBeforePaint = {
  name: 'access-mode-before-paint',
  transformIndexHtml: () => [{ tag: 'script', children: ACCESS_MODE_BEFORE_PAINT, injectTo: 'head-prepend' }]
}

export default defineConfig({
  base: '/moq-wasm/',
  plugins: [react(), accessModeBeforePaint],
  resolve: {
    // The `source` export condition resolves `moq-wasm` to its TypeScript sources.
    conditions: ['source', ...defaultClientConditions]
  },
  server: {
    fs: {
      allow: ['..', '../..']
    }
  },
  build: {
    outDir: 'dist',
    rolldownOptions: {
      input: globSync('{index.html,examples/**/index.html}', { cwd: import.meta.dirname }).map((page) =>
        resolve(import.meta.dirname, page)
      )
    }
  }
})
