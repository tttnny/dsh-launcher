import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'

// The Library page's tab lives in the route query, so every tab is a real link
// (`#/library?tab=profiles`) and the tray / native menu can point straight at
// one. The tab is a view over `?tab=`, never local state — the URL is the claim
// about what is on screen.

export type LibraryTab = 'versions' | 'homes' | 'profiles'

const VALID: LibraryTab[] = ['versions', 'homes', 'profiles']

export function useLibraryTab() {
  const route = useRoute()
  const router = useRouter()

  const current = computed<LibraryTab>(() => {
    const raw = route.query.tab
    const value = typeof raw === 'string' ? raw : ''
    return (VALID as string[]).includes(value)
      ? (value as LibraryTab)
      : 'versions'
  })

  function select(tab: LibraryTab) {
    if (tab === current.value) return
    void router.replace({ name: 'library', query: { ...route.query, tab } })
  }

  return { current, select, tabs: VALID }
}
