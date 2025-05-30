import { defineConfig } from 'vite'

export default defineConfig({
  base: '/moq-wasm/',
  build: {
    outDir: 'dist',
    rollupOptions: {
      input: {
        main: 'examples/media/index.html'
      }
    }
  }
})
