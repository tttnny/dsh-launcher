import { createRouter, createWebHashHistory } from 'vue-router'

const router = createRouter({
  // Hash history keeps the Tauri webview happy without server rewrites.
  history: createWebHashHistory(),
  routes: [
    { path: '/', name: 'home', component: () => import('@/views/Home.vue') },
    { path: '/instances', redirect: '/' },
    { path: '/instances/:id', name: 'instance-edit', component: () => import('@/views/InstanceEdit.vue') },
    { path: '/homes', name: 'homes', component: () => import('@/views/Homes.vue') },
    { path: '/profiles', name: 'profiles', component: () => import('@/views/Profiles.vue') },
    // The plugin manager is not a page of its own any more (spec: no top-level
    // nav entry) — it lives inside the Profile list, as the action on the row
    // it manages. It is still a real route, because opening it is a real
    // navigation and a hash link must not lie about which page it shows; what
    // that route renders is the Profile list with the named Profile's manager
    // open, which is exactly the state the page would be in had the row been
    // clicked. There is no second copy of the list: the route reuses the view.
    {
      path: '/plugins',
      name: 'plugins',
      component: () => import('@/views/Profiles.vue'),
    },
    { path: '/versions', name: 'versions', component: () => import('@/views/Versions.vue') },
    // Task progress is not a page: installing a version is the only long task
    // and its progress belongs to the version it installs (see Versions.vue).
    // The route stays so `/tasks` remains a working legacy link.
    { path: '/tasks', redirect: '/versions' },
    { path: '/settings', name: 'settings', component: () => import('@/views/Settings.vue') },
    { path: '/setup', name: 'setup', component: () => import('@/views/Setup.vue') },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

export default router
