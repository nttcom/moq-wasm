import { defaultClientConditions, defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { globSync } from 'node:fs'
import { resolve } from 'node:path'

export default defineConfig({
  base: '/moq-wasm/',
  plugins: [react()],
  resolve: {
    // The `source` export condition resolves `moq-wasm` to its TypeScript sources.
    conditions: ['source', ...defaultClientConditions],
    alias: {
      '@moqt': resolve(import.meta.dirname, 'lib/moqt')
    }
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
