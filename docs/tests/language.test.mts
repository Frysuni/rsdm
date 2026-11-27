import { test } from 'node:test'
import assert from 'node:assert/strict'
import { preferredLanguage } from '../.vitepress/theme/language.ts'

test('browser language preferences select the first supported language', () => {
  assert.equal(preferredLanguage(['ru-RU', 'en-US'], null), 'ru')
  assert.equal(preferredLanguage(['en-GB', 'ru'], null), 'en')
  assert.equal(preferredLanguage(['de-DE', 'RU'], null), 'ru')
  assert.equal(preferredLanguage(['fr-FR'], null), 'en')
  assert.equal(preferredLanguage([], null), 'en')
})

test('an explicit language choice overrides the browser', () => {
  assert.equal(preferredLanguage(['ru-RU'], 'en'), 'en')
  assert.equal(preferredLanguage(['en-US'], 'ru'), 'ru')
  assert.equal(preferredLanguage(['ru-RU'], 'invalid'), 'ru')
})
