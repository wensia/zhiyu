# ADR-0001：服务端权威 + 瘦客户端

- 状态：**Accepted**（2026-08-09）
- Supersedes：`docs/module-architecture.md` 的 **D1、D4、D5** 与 **§1.1**
- 相关计划：`docs/plans/thin-client-migration.md`

## 背景

知余原定为 local-first 个人工作空间：桌面端内嵌 Axum + 本地 SQLite，Vault 中签名加密的
事件为规范真相，Git 作为不可信复制传输在设备间同步。该方向已在独立 worktree
`products/zhiyu-local-first-m1` 施工（platform-kernel 约 9,626 行 + Finance/Shard/
Collections/Debt 四模块 + Scheduler + legacy-importer），但**至今没有真实数据**。

与此同时，`main` 线的 Web/Axum/SQLite 服务已部署在 `zhiyu.askfish.net` 并承载真实账本。

促成本次转向的需求：**电脑离线时，账本仍需对外可用**——多端访问，以及 hermes、
openclaw 等外部系统的接入。这要求一个始终在线、能读写明文数据的节点。

## 决定

采用**服务端权威 + 瘦客户端**架构：

1. 服务器是唯一权威副本，持明文 SQLite，对外提供 `/api/v1`。
2. Tauri 桌面端退化为指向远程 URL 的 WebView 壳，不内嵌后端、不持本地库。
3. 外部系统通过 API 密钥（`Authorization` header）接入同一套 API。
4. 备份由服务端承担：`VACUUM INTO` 快照 + restic 加密投递到对象存储。
5. 离线能力由前端缓存与异步同步队列提供，而非本地权威副本。

## 被推翻的原有决策

| 原决策 | 原文要旨 | 新状态 |
|---|---|---|
| **D1** | 知余是 local-first 个人工作空间 | **作废**。改为服务端权威的自部署应用 |
| **D4** | V1 拓扑是 single-owner、multi-device、multi-vault | **作废**。改为单用户、单权威库、多客户端 |
| **D5** | Vault 中签名加密的 control records / events / 对象共同构成规范真相；Git 是不可信复制传输 | **作废**。服务端 SQLite 即规范真相，无加密事件层，无 Git 同步 |
| **§1.1 非目标** | 明确不做「Web 多用户中央服务」、禁止「未经签名和加密的远端业务数据」 | **作废**。中央服务正是本决定的目标形态；远端数据为明文 |

`module-architecture.md` 中依赖上述四项的下游内容（§3 目标架构分层、§6.1 分层真相、
§8 同步/冲突/恢复、§8.6 导出与备份的 canonical recovery 部分）随之失效。D2、D3、D7、
D8 关于模块边界与财务语义的部分不受本 ADR 影响，但其适用范围收缩到单进程服务端。

## 代价（已知并接受）

- **服务器持明文**。被入侵或云厂商侧泄露即等于账本全裸。原设计的「远端不可信」假设不再成立。
- **可用性依赖服务器**。阶段一完成后到阶段二上线前，断网期间完全无法记账；这是相对
  当前桌面版（本地库，永远可用）的净倒退。
- **沉没成本**。`zhiyu-local-first-m1` 的约 9,626 行 platform-kernel 及四个模块不再演进。
  代码保留在原 worktree，不删除，但不再是产品主线。
- **8-07 的设计文档失效**。「跨模块时间线与日历模块」整份建立在 local-first 架构之上，
  需另行 supersede。

## 为何接受

- local-first 那套复杂度（事件溯源、签名加密、设备撤权、冲突 resolver、确定性 replay）
  的存在理由是「多设备离线协同 + 不可信远端」。服务器一旦成为持明文的权威节点，这些
  复杂度全部失去用武之地，而它们对一个单人账本严重超配。
- 真实数据已经在服务器上，转向不需要数据迁移。
- 外部系统接入在 local-first 模型下需要设计能力受限的独立身份、部分解密授权等机制；
  在中心化模型下就是一个 API 密钥。

## 未来若要回到 local-first

必须另立 ADR 并正视：服务端明文库需要重新导入为加密事件流、已发放的 API 密钥需要撤销、
客户端需要重新获得密钥材料。不得复用本 ADR 的任何结论声称兼容。
