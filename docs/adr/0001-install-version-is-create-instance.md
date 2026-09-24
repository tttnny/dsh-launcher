# 装版本 ≡ 建 1:1 实例

安装一个 DSH 版本没有独立路径：`start_install_version_task` 直接委托给
create-instance 任务（name = version，dedicated HOME）。UI 不提供「只装版本」或
「手动建实例」入口，版本列表页即实例创建入口。当初的选择：为每个版本维护一个
1:1 实例（HOME、profile、端口一应俱全）使「装完即可启动」零额外步骤，代价是
无法存在不被实例引用的版本；死代码清理时已把无调用的 create/copy/delete
instance 命令移除，防止两条入口分叉。若未来要支持「无实例版本」（如仅为插件
开发装多版本），需重新设计版本页的交互并回退本决策。
