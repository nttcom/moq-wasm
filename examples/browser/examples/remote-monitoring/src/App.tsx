import './App.css'
import { MonitoringRoom } from './components/MonitoringRoom'
import { PublisherApp } from './publisher/PublisherApp'
import type { CameraId } from './types/monitoring'
import { ALL_CAMERA_IDS, RELAY_PRESETS } from './types/monitoring'

const DEFAULT_LOCATION = 'my-building'
const DEFAULT_RELAY = RELAY_PRESETS[0].value

function readParams() {
  const p = new URLSearchParams(window.location.search)
  const cam = p.get('cam')
  return {
    mode: p.get('mode') ?? 'monitor',
    location: p.get('location') ?? DEFAULT_LOCATION,
    relay: p.get('relay') ?? DEFAULT_RELAY,
    camId: (cam && ALL_CAMERA_IDS.includes(cam as CameraId) ? cam : null) as CameraId | null
  }
}

function App() {
  const { mode, location, relay, camId } = readParams()

  if (mode === 'publisher') {
    return <PublisherApp location={location} camId={camId ?? ''} relayUrl={relay} />
  }

  return <MonitoringRoom location={location} relayUrl={relay} />
}

export default App
