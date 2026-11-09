/// The MoQT draft the examples speak, chosen on the example list and kept in localStorage, so every
/// example follows it.

import type { MoqVersion } from 'moq-wasm'

const VERSION_KEY = 'moqt.version'
const DEFAULT_VERSION: MoqVersion = 'draft-18'

function readMoqtVersion(): MoqVersion {
  const stored = localStorage.getItem(VERSION_KEY)
  return stored === 'draft-14' || stored === 'draft-18' ? stored : DEFAULT_VERSION
}

export const moqtVersion: MoqVersion = readMoqtVersion()

export function writeMoqtVersion(version: MoqVersion): void {
  localStorage.setItem(VERSION_KEY, version)
}

/// For `MoqSession.connect`'s `versions`: only the chosen draft, so a relay that does not accept it
/// fails the connect instead of falling back to the other.
export function offeredVersions(): MoqVersion[] {
  return [moqtVersion]
}
