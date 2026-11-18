import { expect, test } from '@playwright/test'
import { resolveRecorderUrl } from './recorderApi'

test('a configured recorder wins over the local default', () => {
  // Act / Assert
  expect(resolveRecorderUrl('?recorderUrl=http://127.0.0.1:9096/', 'https://localhost:4433')).toBe(
    'http://127.0.0.1:9096'
  )
})

test('a local relay implies the local recorder and any other one the deployed recorder', () => {
  // Act / Assert
  expect(resolveRecorderUrl('', 'https://127.0.0.1:4433')).toBe('http://localhost:8096')
  expect(resolveRecorderUrl('', 'https://relay-1.moqt.research.skyway.io:443')).toBe(
    'https://recorder.moqt.research.skyway.io'
  )
})
