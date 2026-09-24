# 实例 URL 永远走系统浏览器

打开实例 = 经 `open that` 用系统浏览器打开实例 URL，与 `open_external` 同策略
（先 trim 再只放行 http 与 https）。不存在任何实例 webview：`open_instance_window`
保留命令名和只传 id 的签名，后端从 running 表查 URL。曾被内嵌 webview 的方案
取代又回退——系统浏览器获得 DSH 自身的完整会话/ Cookie 语义，而 webview 里
remote-web-ui 的 30 天会话 Cookie 不可靠。
