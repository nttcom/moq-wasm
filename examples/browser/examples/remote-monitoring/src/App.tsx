import './App.css'
import { MonitoringRoom } from './components/MonitoringRoom'
import { PublisherApp } from './publisher/PublisherApp'
import { NAME_PATTERN } from './types/monitoring'
import { buildLocalRelayPresets } from '../../../utils/relayPresets'
import { urlFromQuery } from '../../../utils/serviceUrl'
import { randomName } from '../../../utils/randomName'

const DEFAULT_RELAY = buildLocalRelayPresets()[0].value

function nameParam(params: URLSearchParams, key: string): string | undefined {
  const value = params.get(key)?.trim()
  return value && NAME_PATTERN.test(value) ? value : undefined
}

/// A URL without a valid `location` gets a random one; the page writes it back
/// into the URL so its link can be shared.
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
