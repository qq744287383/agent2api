# 固定上游 IP（WorkBuddy 国际版）· 设计文档

> 状态：待确认。确认后按「先后端链路、再面板」两步实现。
> 术语见 [`GLOSSARY.md`](../GLOSSARY.md)；取舍理由见 [ADR-0001](adr/0001-pinned-upstream-ip.md)。

## 1. 需求回顾

`workbuddy-intl` 的上游是 `https://www.workbuddy.ai`，DNS 结果动态。要让用户能把该域名固定到一组自选 IP，并在链路变差时自动换 IP / 换账号。

已确认的边界：

| 项 | 结论 |
|---|---|
| 命名 | 固定上游 IP / pinned upstream IP |
| 超时口径 | 「等待响应」= 请求发出 → 首个数据帧，即 `LogPhase::Waiting` 那一段 |
| 轮询模型 | 粘滞：一个 IP 用到失效为止，不做每请求轮换 |
| 触发后 | 切 IP 与换账号**同时做**，不原地重试 |
| 生效范围 | 该 provider 的全部上游请求（对话 / 目录 / 余额 / 续期 / 保活） |
| 登录 | **不**套固定 IP（登录走另一通道、无账号上下文） |
| 状态码 | 叠加去重；只比对上上游真实返回的码，网关合成的 502 不算 |
| 终点 | 唯一预算是全局换号次数（默认 5）；IP 可轮多圈 |
| 全失败 | 返回最后一次上游错误 |
| 代理冲突 | 出网代理优先，固定 IP 不生效并记日志 |
| IP 形态 | 仅 IPv4，端口固定 443 |
| 端点 | 只作用于 `www.workbuddy.ai`，staging 不参与 |
| 组当前 IP | 持久化，重启继用 |
| 账号级 IP | 自动切换时**直接改写**账号记录 |
| 改 IP 列表 | 组当前 IP 与所有账号独立 IP 全部重置为跟随组 |
| 手动切组 IP | 只对新请求生效，不打断在途流 |
| 日志 | 结构化字段，每次切换各记一行 |
| i18n | 六语全覆盖，跑完整构建链 |

## 2. 数据模型

### 2.1 配置（`kv` 表，与 `retry` / `routing` 同一套四段式）

单一配置键 `pinnedUpstreamIp`，值是对象：

```jsonc
{
  "enabled": false,              // 开关
  "ips": ["1.2.3.4", "5.6.7.8"], // IPv4 列表；空数组 = 未配置
  "waitTimeoutSeconds": 0,       // 0 = 跟随全局 headersTimeoutSeconds
  "statusCodes": [502],          // 切换状态码
  "currentIp": "1.2.3.4"         // 组当前 IP（持久化）；不在 ips 里时视为无效
}
```

约束：

- `waitTimeoutSeconds` **只能比全局更严**：`0` 表示跟随全局；非 0 时取 `min(填的值, 全局值)`，超过全局值不报错但按全局值生效。
- `statusCodes` 默认 `[502]`，合法范围 100–599，上限 50 项（沿用 `RETRY_MAX_NO_RETRY_CODES` 的口径）。
- `ips` 每项必须是合法 IPv4 字面量，去掉重复项，上限 50 项。
- 读取口径沿用 `parse.rs` 的契约：缺字段 / 类型错 / 越界 → 回落默认值，**不报错、不 clamp 到边界**。
- 键名不得与 `db::schema::RESERVED_KV_KEYS` 冲突。

### 2.2 账号

`StoredAccount.fields` 新增一个可选字符串 `pinnedUpstreamIp`。

- 缺失 / 空串 = 跟随组。
- 账号记录本就是整条 JSON（`data` 列 AES-GCM 加密），**不需要 schema 迁移**。
- 需要三处改动：`state.rs` 的 accessor 对、`store_crud.rs::apply_patch` 的校验分支、`store_view.rs::public_account` 的注入（三个导出必须同形）。

## 3. 后端改动点

### 3.1 egress：带固定 IP 的客户端

```rust
// core/egress.rs
pub fn client_for_pinned(
    proxy: Option<&ResolvedProxy>,
    pin: Option<PinnedUpstream>,   // { host: &str, ip: IpAddr }
) -> Arc<reqwest::Client>
```

- `cache_key` 追加 `|p:<ip>`；无固定 IP 时保持原键（存量调用完全不变）。
- `base_builder` 里加 `.resolve(host, SocketAddr::new(ip, 443))`。
- **代理优先**：`proxy.is_some()` 时忽略 `pin`，并让调用方拿到一个「未生效」标记，用于写日志。
- 现有 `client_for` 保留为 `client_for_pinned(proxy, None)` 的薄包装，避免动 40 处调用点。

### 3.2 两条发送路径的接线

| 路径 | 入口 | 改动 |
|---|---|---|
| 对话 / 保活 | `upstream::request::send_chat_request` | `TransportRequest` 增加 `pinned: Option<PinnedUpstream>`；由 `provider_loop` 在构造时按「账号独立 IP → 组当前 IP」填入 |
| 目录 / 余额 / 续期 | `auth_http::send_raw` | 签名增加同一个可选参数（或改用一个携带 egress 描述的结构体，避免继续加参数） |

`send_raw` 拿不到 provider 身份，因此由**调用方**（`models::refresh_with_current_account`、`billing::call_billing`、`auth::refresh_session`）按账号算出 pin 传进去 —— 这三处都已经有账号记录在手。

### 3.3 组当前 IP 的状态机

模块 `core/upstream/upstream_ip_pin.rs`（新文件）：

```rust
/// 本轮请求内的 IP 前进游标：从组当前 IP 之后开始，轮完一圈回到头部。
pub struct IpCursor { ips: Arc<[IpAddr]>, start: Option<usize>, at: Option<usize> }
impl IpCursor {
    pub fn new(ips: Arc<[IpAddr]>, current: Option<IpAddr>) -> Self;
    pub fn next(&mut self) -> Option<IpAddr>;
}
```

- 组当前 IP（`current_ip`）是**持久状态**，只在「首次选定 / 手动切换 / 自动切换成功」时写回配置。
- 自动切换在请求链内推进 `IpCursor`；`current_ip` 是否跟着改写，取决于该轮是否由组策略驱动（账号策略那一支改写的是账号字段）。

### 3.4 转发链的新动作

在 `provider_loop.rs` 的错误处置里，于既有三档（429 限额 / 401 续期 / 普通错误）之外新增一条判据，位置在 `send_with_retry` 返回 `UpstreamRequestError` 之后、`transport_retry_advice` 之前：

```
若固定 IP 已启用，且
   (a) 失败是「等待响应超时」，且填写的超时已生效，或
   (b) 上游真实返回的状态码 ∈ 切换状态码
则：
   - 跳过同账号原地重发（不消耗 resend 预算）
   - 前进 IP（账号策略则改写该账号的 pinnedUpstreamIp；组策略则推进 IpCursor 并写回 current_ip）
   - 换下一个账号（消耗一次换号预算；用尽则把最后一次上游错误返回客户端）
   - 每次切换记一行结构化日志
```

超时判定需要区分两种 502：`UpstreamRequestError.reason` already 形如 `等待响应超时(N秒)` / `连接中超时(N秒)` / `上游连接失败`，直接按 `reason` 前缀判定，无需新增错误类型。

### 3.5 配置变更即重置

设置保存时：

1. 校验（开关打开时 IP 列表必须非空且每行合法）。
2. 若 `ips` 与旧值不同 → 把所有 `workbuddy-intl` 账号的 `pinnedUpstreamIp` 清空，并从新列表里挑一个写入 `currentIp`。
3. 若只是开关 / 超时 / 状态码变化 → 不动账号绑定。

### 3.6 日志

复用现有的结构化通道 `logging::log_event(level, category, message, Some(json!({...})))`，category 用 `upstream`：

```jsonc
{
  "kind": "upstream_ip_switch",
  "attempt": 2,                      // 本次请求里的第几次切换
  "reason": "timeout" | "status",    // 超时 / 状态码
  "status": 502,                     // reason=status 时有值
  "fromIp": "1.2.3.4", "toIp": "5.6.7.8",
  "fromAccount": "user-…", "toAccount": "user-…",
  "scope": "group" | "account",
  "provider": "workbuddy-intl"
}
```

- 每次切换各记一行。
- 「代理优先导致固定 IP 未生效」也走同一通道，`kind` 用 `upstream_ip_pin_ignored`，每次请求只记一次（避免刷屏）。
- UI 侧在 `logs-panel.tsx` 的 `extraText()` 里加一个分支，渲染成 `1.2.3.4 → 5.6.7.8`。

## 4. 面板改动点

### 4.1 组菜单项

`accounts-panels.tsx` 的 `GroupMenu`（`:111-143`）新增一项「固定上游 IP」，仅在 `group.provider === 'workbuddy-intl'` 时渲染。文案不带省略号（该文件既有约定）。

### 4.2 模态框

新建 `accounts-dialog-wbintl.tsx`，照 `proxies-dialogs.tsx` 的 `ProxyModal` 形态（Dialog + 表单 + Switch + 保存）：

- IP 组：多行文本域，一行一个，保存时逐行校验。
- 超时：数字输入，单位秒，默认 0，提示「0 = 跟随设置页的等待响应超时（当前 N 秒）」。
- 状态码：文本输入，英文逗号分隔，默认 `502`。
- 开关：Switch。
- 保存：校验通过才提交；成功后提示，并让面板里账号列的下拉恢复为「跟随组」。

挂在 `accounts-store.ts` 的 `AccountsDialog` 联合类型上（新增一支），由 `accounts-dialogs.tsx` 的 `AccountsDialogs()` 分发。

### 4.3 组标题后的当前 IP

`GroupBandRow`（`accounts-panels.tsx:299-343`）在 `.grp-name` 之后插入：

- 当前 IP 文字（等宽字体，便于核对）。
- 一个小图标按钮 ⟳，点击手动切下一个。

两者仅在开关打开时渲染。窄屏那套 sticky 按钮链（`mobile.css:1155-1161`）需要复核，避免挤进 `right: 0 / 30px` 的既有锚点。

### 4.4 账号表新列

按 `accounts-columns.ts` 的五处登记加一列（列表 + `DEFAULTS` + `ColGroup` + `TableHead` + `cell()`），列内是下拉：

- 首项「跟随组（当前 1.2.3.4）」。
- 其余项是 IP 列表。
- 仅开关打开时渲染（关闭时整列隐藏）。

写侧走 `updateAccount(id, { pinnedUpstreamIp })`，与 `ProxyCell` 的 `applyProxyPick` 同形。

### 4.5 i18n

新增文案走完整链路：源码 `t()` → `scan.mjs` → 填 en / ja / ko / pt-BR（zh-Hant 由 opencc 生成）→ `i18n build` → `ui-islands build`。

## 5. 实现顺序

### 第一步：后端链路（可独立验收）

1. 配置组：`types.rs` / `parse.rs` / `mod.rs` 快照与读写 / `sql.rs`（无需新表）。
2. 账号字段：accessor + `apply_patch` + `public_account`。
3. `egress::client_for_pinned` + 缓存键。
4. 两条发送路径接线（对话 / `send_raw` 的三个调用方）。
5. 转发链新动作 + 日志。
6. 单元测试：IP 校验与去重、游标轮转、超时判定、状态码判定、代理冲突时 pin 被忽略。

验收：直接调 `/v1/chat/completions`，用 `--resolve` 之外的方式可观察 —— 请求日志里出现固定 IP 与切换记录；把第一个 IP 填成不可达地址，请求应自动切到第二个并成功。

### 第二步：面板

7. 组菜单项 + 模态框。
8. 组标题后的 IP 与切换按钮。
9. 账号表新列。
10. i18n 六语 + 重建产物。
11. 手动验收：开关关闭时不显示任何新 UI；打开后保存空列表被拦；改列表后所有账号回到「跟随组」。

## 6. 验收标准

- [ ] 开关关闭时，面板无任何新增 UI，请求行为与现在完全一致。
- [ ] 开关打开且填了 2 个 IP：请求全部走第一个 IP（可在请求日志与抓包里确认）。
- [ ] 把第一个 IP 填成不可达：请求自动切到第二个 IP 并成功，日志里有逐次切换记录。
- [ ] 超时设为 5 秒：上游故意挂住时，5 秒后切 IP + 换账号（而不是等满全局 300 秒）。
- [ ] 上游返回 502：命中切换状态码，切 IP + 换号继续试；换号 5 次用尽后返回最后一次上游错误。
- [ ] 账号配了出网代理：固定 IP 不生效，日志里有一条「未生效」记录，请求照常成功。
- [ ] 手动点组标题的 ⟳：当前 IP 前进一个，在途流不中断。
- [ ] 改 IP 列表（增删一项）：组当前 IP 与所有账号的独立 IP 全部重置为跟随组。
- [ ] 重启网关：组当前 IP 保持不变。
- [ ] `cargo check` 与三个前端构建产物均通过 CI 的逐字节比对。

## 7. 风险与未决

- **超时语义的重复**：全局已经有 `headersTimeoutSeconds`。本功能引入第二个更严的上限，用户在设置页与模态框会看到两个「超时」。缓解：模态框里直接显示「当前全局值 N 秒」，0 即跟随。
- **账号级 IP 被改写**：系统会改写用户的显式选择（见 ADR 决定 6）。若日后用户反馈困惑，可改为「只在本次请求内前进」。
- **IP 列表的正确性无法验证**：不校验 IP 是否属于该域名。填错的表现是证书不匹配或超时，靠切换兜底。
- **未覆盖的路径**：登录与「模型测试」等不携带账号上下文的探测路径不走固定 IP。
