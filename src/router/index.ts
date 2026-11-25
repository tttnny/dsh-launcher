import { createRouter, createWebHashHistory } from 'vue-router'

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
    {
      path: '/install',
      name: 'install',
      component: () => import('@/views/Install.vue'),
    },
    {
      path: '/data',
      name: 'data',
      component: () => import('@/views/Data.vue'),
    },
    {
      path: '/machines',
      name: 'machines',
      component: () => import('@/views/Machines.vue'),
    },
    {
      path: '/machines/:id',
      name: 'machine-detail',
      component: () => import('@/views/MachineDetail.vue'),
    },
    {
      path: '/settings',
      name: 'settings',
      component: () => import('@/views/Settings.vue'),
    },

    {
      path: '/library',
      redirect: (to) => (to.query.tab === 'versions' ? '/install' : '/data'),
    },
    { path: '/versions', redirect: '/install' },
    { path: '/tasks', redirect: '/install' },
    { path: '/homes', redirect: '/data' },
    { path: '/profiles', redirect: '/data' },
    { path: '/hosts', redirect: '/machines' },
    { path: '/setup', redirect: '/machines/host-local' },
    { path: '/plugins', redirect: '/' },
    { path: '/home', redirect: '/' },
    { path: '/instances', redirect: '/' },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

export default router
