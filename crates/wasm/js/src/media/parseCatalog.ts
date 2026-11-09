import { parse_msf_catalog_json } from '../../pkg/moqt'

/// Parses an MSF catalog (draft-ietf-moq-msf-01 §5) and throws when it is not
/// one. The wasm module must be loaded: `loadMoqt`, or any `MoqSession.connect`.
export function parseCatalog(json: string): unknown {
  return parse_msf_catalog_json(json)
}
