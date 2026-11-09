import './App.css'
import { MonitoringRoom } from './components/MonitoringRoom'
import { PublisherApp } from './publisher/PublisherApp'
import { NAME_PATTERN, RELAY_PRESETS } from './types/monitoring'
import { urlFromQuery } from '../../../utils/serviceUrl'
import { randomName } from '../../../utils/randomName'

const DEFAULT_RELAY = RELAY_PRESETS[0].value

/// The query's value when it is a namespace element, else undefined.
function nameParam(params: URLSearchParams, key: string): string | undefined {
  const value = params.get(key)?.trim()
  return value && NAME_PATTERN.test(value) ? value : undefined
}

/// The location is the page's room: given by the URL that was shared, or made
/// here and written into the URL so this page's link can be shared.
function readParams() {
  const p = new URLSearchParams(window.location.search)
  return {
    mode: p.get('mode') ?? 'monitor',
    location: nameParam(p, 'location') ?? randomName(),
    relay: urlFromQuery(window.location.search, ['relay']) ?? DEFAULT_RELAY,
    camId: nameParam(p, 'cam') ?? ''
  }
}

function App() {
  const { mode, location, relay, camId } = readParams()

  if (mode === 'publisher') {
    return <PublisherApp location={location} camId={camId} relayUrl={relay} />
  }

  return <MonitoringRoom location={location} relayUrl={relay} />
}

export default App
