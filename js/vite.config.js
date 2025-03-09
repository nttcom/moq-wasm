import { defineConfig } from 'vite'

export default defineConfig({
  build: {
    rollupOptions: {
      input: {
        main: 'src/pages/index.html',
        about: 'src/pages/about.html'
      }
    }
  }
})
