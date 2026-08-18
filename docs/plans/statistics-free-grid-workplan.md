# 统计仪表盘 · 自由留白改造工作清单（2026-08-18）

把统计页从「系统替你排版」改成「组件待在你放的位置」。设计与取舍见本文 §0，
逐阶段施工令见 §P1–§P5，进度表在文末。

**执行约定**：Codex 分批施工、Claude 逐批验收。每批只跑本文给出的**定向命令**，
不要跑 `pnpm check` / `cargo test --workspace` / 任何 playwright（长任务在全量门禁上会卡死，
且沙箱不允许监听 TCP）——全量门禁与 e2e 由 Claude 跑。所有路径写绝对路径。
每阶段独立提交，不要跨阶段混提交。

---

## §0 背景与已定取舍

### 病灶

`apps/web/src/features/statistics/dashboard-grid.tsx:109-124` 是
`compactType="vertical"` + `preventCollision=false`，于是：

- 组件永远自动上浮——拖到下面留个空位，松手就飞回去，留白权不在用户手里；
- 拖动 A 会推挤 B/C/D——只想动一个，结果动了一片；
- 新组件固定落到 `x:0, y=最底行`（`statistics-dashboard-workspace.tsx:181`），放不进特意留出的位置。

尺寸自由不缺：`minW/minH` 有、`maxW/maxH` 全仓库不存在，后端只校验「不越 12 列」
（`apps/api/src/dashboards.rs:158-166`）。**要还的是位置与留白，不是尺寸。**

### 已定取舍

用户拍板：尺寸**按格连续**（不做 S/M/L 档位化）；**有限宽 12 列 + 纵向无限滚动**
（不做缩放平移的无限画布）。

Council 裁决（codex/antigravity/grok/kimi 四家，报告
`/Users/panyuhang/.council/zhiyu/council-20260818-100759/viewer.html`）：

| 议题 | 结论 |
|---|---|
| 删除留洞、布局稀疏 | 接受，另给显式「整理」动作 |
| 拖动碰撞 | `preventCollision=true`（放不下不给放），**不要 iOS 式推挤** |
| resize 方向 | 开 `e`/`s`/`se`；不开 `n`/`w` |
| 编辑态网格底纹 | 画，低对比功能底纹 |
| 移动端 | 维持只读 |

### 三个决定性结论（读 react-grid-layout 1.5.4 源码得出，施工时不要推翻）

1. **`compactType={null}` 单独改不够，必须配 `preventCollision={true}`。**
   `utils.js:339`：`compactType=null` + `allowOverlap=false` 时 `compact()` 仍会把重叠组件往下推；
   `utils.js:542-545` 的推挤分支会**改写你正在拖的那个组件的 y**（拖到第 5 行落在第 3 行）。
   三件套缺一不可：`compactType={null}` + `preventCollision={true}` + `allowOverlap={false}`。

2. **坐标不用迁移，但 P1 本身就是迁移机制。**
   已存坐标都是 vertical compact 的产物，天然无重叠无越界，`compactType=null` 下原样返回——
   **`0031_dashboards.sql` 的 CHECK 一个字都不用改，不写迁移脚本**。
   但现在没有 `onLayoutChange`，删组件时 PUT 存的是剩余组件的旧坐标，屏幕上看到的却是每次挂载
   重新 compact 的投影：**库里有洞、屏上没有**。关掉 compact 那些洞就全露出来。
   所以先单独发 P1（仍是 vertical compact），面板下次被浏览时自动回写自愈，再发 P2。

3. **补 `onLayoutChange` 有个能永久毁布局的坑。**
   768–1023px 挂载的是 `sixColumnLayout()` 的 6 列坐标（`dashboard-grid.tsx:26-31`），
   裸接 `onLayoutChange` 会把 6 列坐标写回库，**把 12 列布局毁掉**。
   另有挂载即触发（`cloneLayoutItem` 多带 `moved/static` 等键，`deepEqual` 必然不等 →
   每次切页签发一次 PUT）。
   两道防线：`desktop` 闸门 + 以 `JSON.stringify(widgetInputs(widgets))` 为键的 payload 去重
   （后者顺带挡住「保存失败回滚 → 重新 compact → 再触发」的死循环）。

---

## §P1 让「看到的」等于「存的」

**不改变任何交互**，用户零感知。此阶段 `compactType` 仍是 `"vertical"`。

### 施工项

1. `apps/web/src/features/statistics/dashboard-grid.tsx:120-121`
   删掉 `onDragStop` / `onResizeStop`，换成单一 `onLayoutChange`。
   `onLayoutChange` 覆盖 drag/resize/compact 全部路径，保留两个 stop 只会重复触发。
   **闸门必须是 `desktop` 而不是 `editing`**——自愈发生在只读浏览时。

2. `apps/web/src/features/statistics/dashboard-grid.tsx:90-96`（`applyLayout`）
   加空转判定：算出 next widgets 后，若每个 widget 的 x/y/w/h 与入参 `widgets` 完全一致就直接
   return，不调 `onLayoutChange`。这层挡掉挂载回调。

3. `apps/web/src/features/statistics/statistics-dashboard-workspace.tsx:126-138`（`queueWidgetSave`）
   加签名去重：`useRef(new Map<string, string>())`，键 `dashboardId`、值
   `JSON.stringify(widgetInputs(widgets))`；签名相同整体 return。

4. 同文件，`saveTimers`（`:70`）补 flush-on-unmount：
   `useEffect(() => () => flushPendingSaves(), [])`，清掉所有定时器并对挂起项直接调
   `persistWidgets`。现在完全没有 cleanup，切页会丢掉防抖期内的改动。

### 验收（只跑这些）

```
cd /Users/panyuhang/projects/coding/products/zhiyu/apps/web && npx tsc -b --pretty false
cd /Users/panyuhang/projects/coding/products/zhiyu/apps/web && pnpm vitest run src/features/statistics/
```

### 必须新增的单测（`statistics-dashboard-workspace.test.tsx`）

- 挂载后**不**发任何 PUT（现成绊线：`:109` 断言删组件后 PUT 恰好 2 次，多发一次会变 3 次）
- `window.innerWidth = 900` 时挂载**不**发 PUT（防 6 列坐标覆盖 12 列，这条最重要）
- 卸载时挂起的保存立即 flush（`vi.useFakeTimers` + `unmount()`）

---

## §P2 自由留白（主体）

### 施工项

1. `dashboard-grid.tsx:109-124` 的 RGL props：
   - `compactType="vertical"` → `compactType={null}`
   - `preventCollision={false}` → `preventCollision={true}`
   - 新增 `maxRows={200}`（给「纵向无限」一个软天花板，避免拖到 y=9999 造出回不去的页面）
   - 把 `rowHeight={72}` 与 `margin={[16,16]}` 提成模块级常量 `ROW_HEIGHT` / `GRID_MARGIN`，P4 要复用

2. `apps/web/src/features/statistics/utils.ts` 新增纯函数（**放这里而不是组件文件**，
   `react-refresh/only-export-components` + `--max-warnings 0` 不允许组件文件导出非组件）：
   ```ts
   export function findFreeSlot(
     occupied: Array<{ x: number; y: number; w: number; h: number }>,
     w: number, h: number, cols = 12,
   ): { x: number; y: number }
   ```
   `w` 夹到 `[1, cols]`；按 y 从 0 到 `bottom`、x 从 0 到 `cols-w` 扫描，返回首个不与任何已占矩形
   相交的位置；全都放不下则返回 `{ x: 0, y: bottom }`。

3. `statistics-dashboard-workspace.tsx:178-193`（`addWidget`）
   把 `:181` 的 `y = max(y+h)` 与 `:186-187` 的 `x:0, y` 换成 `findFreeSlot(...)`。
   这条是「删组件留洞」的解药：洞会被下一个新组件优先填掉。

4. `apps/web/src/styles.css:911` 附近（`.statistics-grid-region`）与 `dashboard-grid.tsx:101`
   编辑态给区域补底部留白（约 3 行高）。**RGL 容器高度 = bottom(layout) × 行高，且没有拖拽
   自动滚动**——网格填满视口后下面没有落脚处，不补留白「纵向无限」就是假的。

5. `dashboard-grid.tsx:79-87`：`minW/minH` 来自异步的 `widgetTypes`，未就绪时是 `undefined`
   （能拉到 1×1）。兜 `?? 1`。

**不要加 `maxW/maxH`**：w 已被 12 列 CHECK 封顶、h 由 `maxRows` 封顶；加了等于从后门改回档位化，
还会动 `apps/api/src/plugins.rs:35-43` 的 schema，逼出 `api:generate` 与 `registry.ts:45-55`
那份手抄镜像的同步。不加 = 后端零 schema 变更。

### 验收（只跑这些）

```
cd /Users/panyuhang/projects/coding/products/zhiyu/apps/web && npx tsc -b --pretty false
cd /Users/panyuhang/projects/coding/products/zhiyu/apps/web && pnpm vitest run src/features/statistics/
```

### 必须新增的单测（新建 `apps/web/src/features/statistics/utils.test.ts`）

参照仓库已有的同型纯函数测试（`features/transaction-link-picker.test.ts`、
`features/ledger-account.test.ts`）。用例：空面板 → (0,0)；左上有洞 → 填洞；整行占满 → 换行；
宽度超 12 → 夹紧；无处可放 → 落到 bottom。

### 不在本阶段做

e2e（`e2e/statistics-layout.spec.ts`：拖到下方远处 → 断言停住 → reload → 位置一致）由 Claude 补，
Codex 沙箱跑不了 playwright。

---

## §P3 把手：`e` / `s` / `se`

1. `dashboard-grid.tsx` 加 `resizeHandles={["se","e","s"]}`。
   **不开 `n`/`w`**：把手是 20×20 绝对定位盒，顶部三个会从卡片头部（整条拖拽把手）里抠掉两角
   一中点；且 `n`/`w` 要同时改 x/y，在 `preventCollision=true` 下更容易整体回退。
   `draggableCancel`（`:113`）不匹配 `.react-resizable-handle`，无需改。
2. `apps/web/src/styles.css:916`
   现有 `.react-resizable-handle::after` **没有方向选择器**，开三向后三个把手会画同一个右下角标 →
   必须拆成 `-se` / `-e` / `-s` 三条。注意 RGL 对 `-e` 施加 `rotate(315deg)`、对 `-s` 施加
   `rotate(45deg)`，角标画在旋转后的坐标系里。保持 1px `--border-visible` 细线，不加填充与背景；
   命中区可放大到 24px，但可见墨迹不变。

---

## §P4 空位可见 + 一键整理

1. 编辑态网格底纹：列间距 `calc((100% + var(--space-4)) / 12)`、行距 = `ROW_HEIGHT + 16`（88px），
   挂在**区域**上（覆盖 P2 的底部留白）。由 TSX 用 inline style 注入
   `--statistics-grid-row` / `--statistics-grid-col` 两个变量作为单一真相源，并把这两个名字加进
   `scripts/verify-design.mjs` 的 `externalRuntimeTokens`（`--tx-entry-gap`/`--tx-entry-height`
   就是这个先例），否则 token 契约会报未知 token。
2. `utils.ts` 新增 `tidyLayout(widgets)`（约 15 行：按 (y,x) 排序，逐个把 y 上移到不碰撞为止）。
   **不要 deep-import RGL 的 `build/utils`**（未导出、无类型、随时会碎）。
3. 编辑态顶栏加「整理」按钮，`variant="outline"`（**不是 clay**——本页零 clay 填充是设计文档
   §三 的既定约束），点击走同一条 `queueWidgetSave`；配一条带「撤销」action 的 toast
   （复用 `persistWidgets` 已有的可重试 toast 形态，撤销即把整理前的数组再存一次）。

---

## §P5 后端护栏与文档翻案

1. `apps/api/src/dashboards.rs:158-166`（`validate_widget`）加 `y + h > 200` 的 422。
   **必须排在 P2 之后**，否则前端还没有 `maxRows` 钳制，一拖过界就吃 422 + 回滚 toast。
2. `dashboards.rs:586-594` 的整体重叠检测**可选**：P2 之后客户端由 `preventCollision` 保证不产出
   重叠，触发概率极低，而一格重叠会让整次保存 422 且 UI 难解释。倾向宽容就跳过。
3. **不改 `migrations/0031_dashboards.sql`**：SQLite 改表级 CHECK 要 12 步重建表，不值得。
   DB CHECK 当兜底不变量、行数上限当应用层策略，在代码里注明这个非对称是故意的。
4. `dashboards.rs` 的 `mod tests` 补：y 越界被拒（对齐既有
   `widget_validation_rejects_unknown_plugin_and_grid_overflow` 的写法）。
5. 改 `docs/plans/statistics-dashboard-design.md` §三（`compactType`/`preventCollision`/「缩放把手
   = 右下角」）、§五（`:44` 的「自动下移到第一块能放下的位置」重写为 findFreeSlot 语义；编辑态
   变化三处 → 四处，含底纹）、§九（把「自由坐标」从不做里摘出来，保留「无限画布、缩放平移、
   组件重叠」）。**文档正是被翻案的那份，不改它下一个人会照旧文档改回去。**

验收：`cd /Users/panyuhang/projects/coding/products/zhiyu && cargo check -p zhiyu-api`
（`cargo test --workspace` 由 Claude 跑）。

---

## 明确不做

- 不加 `maxW/maxH`（理由见 P2）。
- 不动 `sixColumnLayout` / `singleColumnLayout` 的只读降级（`dashboard-grid.tsx:26-40`）——
  它们是只读投影，与自由摆放正交；唯一要保证的是降级坐标永不写回库。
- 不引入单 widget 的 PATCH：现有「防抖 + 全量 PUT + 回滚 + 幂等键」是本仓库验证过的模式，
  加新写入路径只会多一套失败语义。
- 不做键盘替代（设计文档 §八 的「上移/下移」）——独立议题。

## 进度

| 阶段 | 内容 | 状态 |
|---|---|---|
| P1 | onLayoutChange + flush-on-unmount（= 迁移机制） | 已完成 `e9cce63` |
| P2 | compactType=null + preventCollision + findFreeSlot + 底部留白 | 已完成 `e9cce63` |
| P3 | resize 把手 e/s/se + 三条角标样式 | 已完成 `5c06524` |
| P4 | 编辑态底纹 + 整理动作 + 撤销 | 待施工 |
| P5 | 后端 y 上限 + 文档翻案 | 待施工 |
