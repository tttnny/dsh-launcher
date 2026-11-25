# 本机构建用开发证书签名，保住 macOS 已授予的权限

## 背景

本机自测用的那份 App 装在 `/Applications` 里，正常运行要碰系统保护的东西（比如「完全磁盘
访问」）。macOS 把这类授权记在「这个 App」身上，认的是代码签名的 designated requirement
（DR）。

不带签名身份的构建产物只会拿到临时（ad-hoc）签名，而 ad-hoc 的 DR 就是 cdhash：App 内容
一变 cdhash 就变，DR 跟着变，系统认为这是另一个 App。表现是授权开关还开着、实际却没有
权限——每次重建自测副本都要重新授权一次，用户看到的是「权限莫名其妙没了」。

## 决策

**本机自测安装用钥匙串里的 Apple Development 证书签名，并在安装前验证签名身份与 DR。**

- `scripts/build-local-app.sh` 负责这条路：用证书身份构建，把产物装到
  `/Applications/dsh-launcher.app`。直接跑 `pnpm tauri build` 不负责签名。
- 签名身份从钥匙串动态取（`security find-identity -v -p codesigning`，取第一张 Apple
  Development），不硬编码：换证书自动跟随，也不会把 Apple ID 写进公开仓库。找不到就报错
  退出，并说明用 Xcode 登录 Apple ID 即可生成（免费账号即可）。
- 装之前验证两件事，任何一条不满足就不装：
  1. 产物真是这张证书签的（读 `codesign -dv --verbose=4` 里的 Authority）。不对就补签再
     验一次，还不对就退出——不然自测会静默退回 ad-hoc。
  2. DR 不含 `cdhash`（用 `codesign -d -r-` 读出来检查）。含 cdhash 说明仍是临时签名，
     装上去下次重建授权就失效，直接报错。
- 换自测副本前先退掉正在跑的那份 App，`ditto` 装好后 `codesign --verify --deep
  --strict` 再放行。

## 后果

- 证书签名的 DR 只钉 bundle id 与证书，不含 cdhash，重建后逐字节不变——授权跨更新保留，
  这是本决策的全部意义。
- 本机自测依赖一张可用的开发证书，没有就直接失败。这比「静默退回 ad-hoc、授权在下次重建
  时失效」好查得多。
- 更换应用标识（bundle id）或签名身份，DR 会变，用户可能要重新授权一次。这是证书身份的
  边界，脚本绕不过去。
- 这条只管本机自测副本。发布包走独立流程：CI 里配了签名材料就签名并公证，没配就产出未签名
  dmg，并在发布说明里写清首次打开的方式。

## 备选方案

- **用 ad-hoc 或不签名的产物做自测**：被否，见背景。
- **把证书名写死在脚本里**：被否。换一张证书就要改脚本，而且 Apple ID 会进公开仓库。
