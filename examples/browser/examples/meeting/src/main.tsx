import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import 'moq-wasm/media/player.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
)
