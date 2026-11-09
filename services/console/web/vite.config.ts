import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': process.env.CONSOLE_URL ?? 'http://127.0.0.1:8080'
    }
  }
})
