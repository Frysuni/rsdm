import { onMounted } from 'vue'
import { useData } from 'vitepress'
import DefaultTheme from 'vitepress/theme'
import { preferredLanguage } from './language'
import './style.css'

const languageKey = 'rsdm-wiki-language'

export default {
  extends: DefaultTheme,
  setup() {
    const { site, page } = useData()

    onMounted(() => {
      let stored: string | null = null
      try {
        stored = localStorage.getItem(languageKey)
      } catch {
        // Language selection still works when storage is disabled.
      }

      document.addEventListener('click', (event) => {
        if (!(event.target instanceof Element)) return
        const link = event.target.closest('.VPNavTranslations a[hreflang]')
        if (!link) return

        const language = link.getAttribute('hreflang')?.split('-')[0]
        if (language === 'en' || language === 'ru') {
          try {
            localStorage.setItem(languageKey, language)
          } catch {
            // The language link still navigates to the selected page.
          }
        }
      }, true)

      const base = site.value.base
      const { pathname, search, hash } = location
      const path = pathname.slice(base.length)
      if (!pathname.startsWith(base) || path === 'ru' || path.startsWith('ru/') || page.value.isNotFound) return
      if (preferredLanguage(navigator.languages, stored) === 'ru') {
        location.replace(`${base}ru/${path}${search}${hash}`)
      }
    })
  },
}
