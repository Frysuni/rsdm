export function preferredLanguage(languages: readonly string[], stored: string | null): 'en' | 'ru' {
  if (stored === 'en' || stored === 'ru') return stored

  for (const language of languages) {
    const primary = language.toLowerCase().split('-')[0]
    if (primary === 'en' || primary === 'ru') return primary
  }
  return 'en'
}
