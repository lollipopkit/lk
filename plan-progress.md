# LK VM 重构交接进度

本文只记录当前快照、已验证事实、未完成风险和下一步执行顺序。`plan.md` 是架构契约，不写日常流水账；本文也要保持短小，避免旧 session 历史压过当前事实。

## 当前总体状态

当前主线已经从旧 VM 兼容迁移转为新架构收口。核心路径围绕 `RuntimeVal`、slot-based `HeapStore`、`Instr32`、`Module32Artifact`、共享 runtime state、runtime callable ABI 和 native named stack/map source 展开。

当前项目未发布，不需要保持旧二进制产物、旧 AOT callable bridge、旧 `Val` runtime shell 或旧 `Op` instruction enum 的向后兼容。已删除的旧路径不能作为 fallback 恢复。
