# 仓库须知

## 上游同步

任何"同步上游 / merge upstream / cherry-pick warp-upstream"类任务,**开工前必须先读 [`docs/openwarp-upstream-sync.md`](docs/openwarp-upstream-sync.md)**。该文档包含:

- 当前同步基线 SHA(决定本次要评估哪些 commit)
- **永久黑名单 commit 表**——cloud / codex / OzHandoff / orchestration / cloud_mode / Warp 内部 workflow / STAKEHOLDERS / 上游内部 docs 等已永久跳过的 commit,后续 sync 直接 skip,不要再单独评估
- openWarp 已删/特化模块表(合入若被恢复需手工 `git rm`)
- 标准合并流程(worktree + cherry-pick + `merge=openwarp-ours` 自治区)

完成 sync 后必须更新该文档的"当前同步状态"和"已知黑名单"两节,保持 100% commit 归属。
