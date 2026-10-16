import { createI18n } from 'vue-i18n'

// JSON locale files under src/locales are discovered by
// @intlify/unplugin-vue-i18n (see vite.config.ts): they are pre-compiled at
// build time and hot-reloaded during development. The glob below collects
// them into the runtime messages map.
const modules = import.meta.glob<{ default: Record<string, unknown> }>(
  '../locales/*.json',
  {
    eager: true,
  },
)

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const messages: Record<string, any> = {}
for (const [path, mod] of Object.entries(modules)) {
  const locale = path.replace('../locales/', '').replace('.json', '')
  messages[locale] = mod.default ?? mod
}

export const SUPPORTED_LOCALES = [
  { value: 'zh-CN', label: '简体中文' },
  { value: 'en-US', label: 'English' },
]

export const i18n = createI18n({
  legacy: false,
  locale: 'zh-CN',
  fallbackLocale: 'en-US',
  messages,
})
