import { expect, test } from '@playwright/test'
import { resolveServiceUrl, urlFromQuery } from './serviceUrl'

const URLS = { param: 'botUrl', local: 'http://127.0.0.1:8097', cloud: 'https://bot.example' }

test('the query parameter wins over the relay-derived service', () => {
  // Act / Assert
  expect(resolveServiceUrl('?botUrl=http://127.0.0.1:9000/', 'https://127.0.0.1:4433', URLS)).toBe(
    'http://127.0.0.1:9000'
  )
})

test('a query parameter on a deployed host wins over the relay-derived service', () => {
  // Act / Assert
  expect(resolveServiceUrl('?botUrl=https://bot-2.moqt.research.skyway.io/', 'https://127.0.0.1:4433', URLS)).toBe(
    'https://bot-2.moqt.research.skyway.io'
  )
})

test('a query parameter on any other host is ignored', () => {
  // Arrange
  const urls = [
    'https://attacker.example',
    'https://moqt.research.skyway.io.attacker.example',
    'https://attackermoqt.research.skyway.io',
    'not a url'
  ]

  // Act
  const resolved = urls.map((url) =>
    resolveServiceUrl(`?botUrl=${encodeURIComponent(url)}`, 'https://relay-1.moqt.research.skyway.io:443', URLS)
  )

  // Assert
  expect(resolved).toEqual(urls.map(() => 'https://bot.example'))
})

test('the first query parameter set is the one checked', () => {
  // Arrange
  const names = ['relayUrl', 'url']

  // Act
  const second = urlFromQuery('?url=https://relay-2.moqt.research.skyway.io:443', names)
  const rejectedFirst = urlFromQuery(
    '?relayUrl=https://attacker.example&url=https://relay-2.moqt.research.skyway.io',
    names
  )

  // Assert
  expect(second).toBe('https://relay-2.moqt.research.skyway.io:443')
  expect(rejectedFirst).toBeUndefined()
})

test('a local relay implies the local service and any other one the deployed service', () => {
  // Act / Assert
  expect(resolveServiceUrl('', 'https://localhost:4433', URLS)).toBe('http://127.0.0.1:8097')
  expect(resolveServiceUrl('', 'https://relay-1.moqt.research.skyway.io:443', URLS)).toBe('https://bot.example')
  expect(resolveServiceUrl('', 'not a url', URLS)).toBe('https://bot.example')
})
