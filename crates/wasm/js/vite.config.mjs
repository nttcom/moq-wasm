import { dirname, relative } from 'node:path'
import { defineConfig } from 'vite'

/// With `renderBuiltUrl` returning a string, Vite writes
/// new URL(path, ``+import.meta.url), which a consumer's bundler does not
/// recognize; this drops the ``+.
const plainImportMetaUrl = {
  name: 'plain-import-meta-url',
  enforce: 'post',
  generateBundle(_options, bundle) {
    for (const output of Object.values(bundle)) {
      if (output.type === 'chunk') {
        output.code = output.code.replaceAll('``+import.meta.url', 'import.meta.url')
      }
    }
  }
}

// An app build, not library mode: library mode inlines the wasm and the
// workers as data URLs. An app build drops the entries' exports unless
// `preserveEntrySignatures` keeps them.
export default defineConfig({
  plugins: [plainImportMetaUrl],
  build: {
    assetsInlineLimit: 0,
    modulePreload: false,
    rolldownOptions: {
      input: { 'core/index': 'src/core/index.ts', 'media/index': 'src/media/index.ts' },
      preserveEntrySignatures: 'strict',
      output: { entryFileNames: '[name].js' }
    }
  },
  worker: { format: 'es' },
  experimental: {
    // new URL('<path relative to the chunk>', import.meta.url), the form a
    // consumer's bundler emits as a worker or an asset.
    renderBuiltUrl: (filename, { hostId }) => JSON.stringify(relative(dirname(hostId), filename)).slice(1, -1)
  }
})
