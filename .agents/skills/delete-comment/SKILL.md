---
name: delete-comment
description: 审计仓库注释：把每块注释判为赘述/借口/过时/决策记录，删除前三者，决策记录并入 ADR，借口开 issue，并让注释基线只降不升。由用户手动执行。
disable-model-invocation: true
---

# Delete Comment

把注释当技术债，按可交付的切片一轮轮清掉。每轮结束时：代码少一批注释，ADR 多几条决策，
注释基线只降不升。

## 先读

- RULES.md（本目录）：四类判定、动作映射、ADR 资格、删除与校验的做法。本轮所有裁决按它。
- docs/adr/ 与 GLOSSARY.md：判断「是否已被 ADR 覆盖」，并用词汇表里的话写新 ADR。
- AGENTS.md 的 issue tracker 一节：借口要开的 issue 走 GitHub Issues。
- .agents/skills/domain-modeling/ADR-FORMAT.md：写 ADR 的格式。

## 一轮的步骤

1. 看现状。运行
       node scripts/check-comments.mjs --report
   选本轮范围：一个文件、一组相关文件，或大文件的一段行号。一轮要能在一次 review 里看完。

2. 出清单。用
       node scripts/check-comments.mjs --details
   得到所有注释块；按 RULES.md 的「清单与合并」把相邻的行注释合成语义块。

3. 分类。逐块打开源码读上下文，给出四类之一与动作。过时项必须实际核对引用的文件、
   函数、命令、路由、ADR 编号在仓库里是否还存在。判「决策记录」时套 RULES.md 的三条标准。

4. 处置。
   - 赘述、过时：删除。
   - 借口：先按 AGENTS.md 的 issue tracker 约定开 issue（标题写实现问题，不复述注释），再删。
   - 决策记录：已被现有 ADR 覆盖的直接删；ADR 级的写进 docs/adr/（新 ADR，或给现有 ADR
     加「追加」节）；不够 ADR 级、但删了会让后来人踩坑的局部约束保留，进基线并写进报告。

5. 删注释。按块删除：整行注释连行一起删，行尾注释只删注释文本；删完不留连续空行。
   不改任何代码，不重排代码。

6. 校验。
       node --test scripts/check-comments.test.mjs
       pnpm verify
   git diff 里只应有注释与空行变化；脚本 check-comments 的测试覆盖了注释解析本身。

7. 下调基线。
       node scripts/check-comments.mjs --update
   只允许下降。确需上调（例如新增了需要解释的机器事实）用 --allow-increase，并在 PR 里写理由。

8. 报告。写清楚：本轮删除的行数与块数、四类计数、新增/更新的 ADR、新开的 issue、
   基线前后值、剩余待处理量（--report 的总数）。

## 基线（ratchet）

scripts/comment-baseline.json 按文件记录允许的注释行数。任何文件的新增注释让行数超过基线时
pnpm verify 失败；新文件默认 0 行。基线只能下降，目标是把文件从基线里逐个删掉，直到空文件，
那时 lint 就是硬禁止注释。**2026-11 的清理已经把基线清空，现在就是硬禁止状态**；下一轮如果要
处理的是新出现的注释，按 RULES.md 的四类判定处理后再跑 --update 维持基线为空。

## 不做什么

- 不把局部实现细节塞进 ADR。ADR 要克制，见 RULES.md。
- 不为了行数删掉真正的约束；不确定的块保留（keep）并在报告里列出。
- 不在这一轮改代码逻辑。实现有问题（借口类）就开 issue，让代码按 issue 单独修。
- 不一次清完整个大仓库。ratchet 的意义就是每轮都能交付、review、继续。
