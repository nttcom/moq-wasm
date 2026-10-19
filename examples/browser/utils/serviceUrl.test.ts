import { expect, test } from '@playwright/test'
import { resolveServiceUrl } from './serviceUrl'

const URLS = { param: 'botUrl', local: 'http://127.0.0.1:8097', cloud: 'https://bot.example' }

test('the query parameter wins over the relay-derived service', () => {
  // Act / Assert
  expect(resolveServiceUrl('?botUrl=http://127.0.0.1:9000/', 'https://127.0.0.1:4433', URLS)).toBe(
    'http://127.0.0.1:9000'
  )
})

test('a local relay implies the local service and any other one the deployed service', () => {
  // Act / Assert
  expect(resolveServiceUrl('', 'https://localhost:4433', URLS)).toBe('http://127.0.0.1:8097')
  expect(resolveServiceUrl('', 'https://relay-1.moqt.research.skyway.io:443', URLS)).toBe('https://bot.example')
  expect(resolveServiceUrl('', 'not a url', URLS)).toBe('https://bot.example')
})
