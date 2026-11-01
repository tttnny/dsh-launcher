import { createRouter, createWebHashHistory } from 'vue-router'

// 五个目的地对应五件真实的事：跑实例、装版本、管数据、管机器、改设置。
// 旧路径保留为跳转，历史链接和原生菜单都不会断。

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

    // 旧地址
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
