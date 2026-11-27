import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readdirSync } from 'node:fs'

function markdownPages(directory: URL, prefix = ''): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    if (entry.name.startsWith('.') || ['node_modules', 'public', 'tests', 'ru'].includes(entry.name)) return []
    const path = prefix + entry.name
    if (entry.isDirectory()) return markdownPages(new URL(entry.name + '/', directory), path + '/')
    return entry.name.endsWith('.md') ? [path] : []
  })
}

test('each English page has a Russian counterpart', () => {
  const docs = new URL('../', import.meta.url)
  assert.deepEqual(markdownPages(new URL('ru/', docs)).sort(), markdownPages(docs).sort())
})
