import { createRouter, createWebHashHistory } from 'vue-router'

// Four top-level pages, matching the four things a user actually does: run an
// instance (instances), prepare what instances run (library), manage the
// machines they live on (hosts), and configure the launcher (settings).
// Everything that used to be its own page — HOME directories, profiles, plugin
// management — lives inside Library, because they are all "what an instance can
// be pointed at"; the environment check is reached from the machine it is about.
//
// Legacy paths stay as redirects: they are real hash links in old docs, the
// tray menu, and the native menu bar.
const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      path: '/',
      name: 'instances',
      component: () => import('@/views/Instances.vue'),
    },
    {
      path: '/instances/:id',
      name: 'instance-detail',
      component: () => import('@/views/InstanceDetail.vue'),
    },

    // Library: versions / HOME directories / profiles, chosen by `tab`.
    {
      path: '/library',
      name: 'library',
      component: () => import('@/views/Library.vue'),
    },
    // The plugin manager is a page of its own — a running instance's expanded
    // card opens it. It carries the (HOME, Profile) pair it manages plus the
    // instance it came from in its query, so the link is honest and "返回实例"
    // can land back on that row.
    {
      path: '/plugins',
      name: 'plugins',
      component: () => import('@/views/library/PluginManager.vue'),
    },
    // 主机: the machines themselves — add / edit / test / delete them, and read
    // each one's footprint and environment. Managing a machine lives here, not
    // inside 设置, because a machine is the axis every other page is filed under.
    {
      path: '/hosts',
      name: 'hosts',
      component: () => import('@/views/Hosts.vue'),
    },
    {
      path: '/settings',
      name: 'settings',
      component: () => import('@/views/Settings.vue'),
    },
    {
      path: '/setup',
      name: 'setup',
      component: () => import('@/views/Setup.vue'),
    },

    // --- Legacy paths -------------------------------------------------------
    { path: '/home', redirect: '/' },
    { path: '/homes', redirect: { name: 'library', query: { tab: 'homes' } } },
    {
      path: '/profiles',
      redirect: { name: 'library', query: { tab: 'profiles' } },
    },
    {
      path: '/versions',
      redirect: { name: 'library', query: { tab: 'versions' } },
    },
    {
      path: '/tasks',
      redirect: { name: 'library', query: { tab: 'versions' } },
    },
    { path: '/instances', redirect: '/' },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

export default router
