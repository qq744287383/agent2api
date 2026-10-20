//! GET/PUT /api/pinned-upstream-ip —— 固定上游 IP（账号页 WorkBuddy 国际版组菜单）。
//!
//! 语义与取舍见 `docs/adr/0001-pinned-upstream-ip.md`，术语见仓库根的
//! `GLOSSARY.md`。这一层的职责只有三件：
//!
//!   1. **校验**：IP 逐项必须是 IPv4 字面量、状态码 100–599、超时在范围内；
//!      全部通过才落盘（与 `retry_api` 同一顺序，不留半套设置）。
//!   2. **保存开关的硬约束**：开关打开时 IP 列表必须非空 —— 用户的原话是
//!      「保存按钮点击的时候校验如果开关是开的必须要填写 ip 组」。这条在
//!      读侧也兜了一次（`pinned_upstream_from` 把「开了但没 IP」收成关），
//!      但读侧兜底是给手改库用的，界面上要明确报错而不是静默失效。
//!   3. **列表变更即重置**：IP 列表一变，组当前 IP 与所有账号的独立 IP 全部
//!      回到「跟随组」（ADR-0001 决定 7）。重置动作在这里执行，因为只有这里
//!      同时拿得到「旧列表」与「新列表」。
//!
//! 与 `/api/retry` 的差别：这条**有副作用**（可能改写一批账号记录），所以
//! 重置发生时要记一行日志，让用户知道自己的账号绑定被清掉了。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{
    self, PinnedUpstreamPatch, PinnedUpstreamSettings, FIELD_PIN_CURRENT_IP, FIELD_PIN_ENABLED,
    FIELD_PIN_IPS, FIELD_PIN_STATUS_CODES, FIELD_PIN_WAIT_TIMEOUT_SECONDS, KEY_PINNED_UPSTREAM_IP,
    PIN_MAX_IPS, PIN_MAX_STATUS_CODES, PIN_MAX_WAIT_TIMEOUT_SECONDS, PIN_MIN_WAIT_TIMEOUT_SECONDS,
    RETRY_CODE_MAX, RETRY_CODE_MIN,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// 这一组对应的 provider id（重置账号绑定时用）。
const PROVIDER_ID: &str = "workbuddy-intl";

/// GET /api/pinned-upstream-ip
pub async fn get_pinned_upstream(State(_state): State<ServerState>) -> Response {
    ok_json(pinned_json(&config::pinned_upstream_settings()))
}

/// PUT /api/pinned-upstream-ip
///
/// body `{enabled?, ips?, waitTimeoutSeconds?, statusCodes?, currentIp?}`。
/// 允许部分字段（缺省 / null = 该项不动）。校验通过后写库 → 返回**生效后**
/// 的值（前端直接用响应刷新界面）。
pub async fn put_pinned_upstream(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };

    let current = config::pinned_upstream_settings();
    let mut patch = PinnedUpstreamPatch::default();
    let mut updated = false;

    // ── IP 列表 ────────────────────────────────────────────────
    if let Some(value) = object.get(FIELD_PIN_IPS) {
        if !value.is_null() {
            match parse_ips(value) {
                Ok(ips) => {
                    patch.ips = Some(ips);
                    updated = true;
                }
                Err(message) => return errors::management_error(400, message),
            }
        }
    }
    // ── 开关 ──────────────────────────────────────────────────
    if let Some(value) = object.get(FIELD_PIN_ENABLED) {
        if !value.is_null() {
            match value.as_bool() {
                Some(enabled) => {
                    patch.enabled = Some(enabled);
                    updated = true;
                }
                None => {
                    return errors::management_error(
                        400,
                        format!("{FIELD_PIN_ENABLED} 必须是布尔值（收到: {value}）"),
                    )
                }
            }
        }
    }
    // ── 等待响应超时 ──────────────────────────────────────────
    if let Some(value) = object.get(FIELD_PIN_WAIT_TIMEOUT_SECONDS) {
        if !value.is_null() {
            match parse_wait_timeout(value) {
                Ok(seconds) => {
                    patch.wait_timeout_seconds = Some(seconds);
                    updated = true;
                }
                Err(message) => return errors::management_error(400, message),
            }
        }
    }
    // ── 切换状态码 ────────────────────────────────────────────
    if let Some(value) = object.get(FIELD_PIN_STATUS_CODES) {
        if !value.is_null() {
            match parse_status_codes(value) {
                Ok(codes) => {
                    patch.status_codes = Some(codes);
                    updated = true;
                }
                Err(message) => return errors::management_error(400, message),
            }
        }
    }
    // ── 组当前 IP（组标题后那个切换按钮）──────────────────────
    if let Some(value) = object.get(FIELD_PIN_CURRENT_IP) {
        if !value.is_null() {
            match value.as_str() {
                Some(text) => {
                    patch.current_ip = Some(text.trim().to_string());
                    updated = true;
                }
                None => {
                    return errors::management_error(
                        400,
                        format!("{FIELD_PIN_CURRENT_IP} 必须是字符串（收到: {value}）"),
                    )
                }
            }
        }
    }

    // ── 保存时的硬约束：开了就必须有 IP ───────────────────────
    //
    // 按**合并后**的值判断，而不是只看本次提交：允许「先传 ips、再单独传
    // enabled=true」这种分两步的写法（组标题的切换按钮只改 currentIp）。
    let next_enabled = patch.enabled.unwrap_or(current.enabled);
    let next_ips: &[String] = match &patch.ips {
        Some(ips) => ips,
        None => &current.ips,
    };
    if next_enabled && next_ips.is_empty() {
        return errors::management_error(
            400,
            "固定上游 IP 已开启，请至少填写一个 IP 组地址（一行一个）",
        );
    }

    if updated {
        let ips_changed = config::set_pinned_upstream(patch);
        if ips_changed {
            // 列表一变，所有账号的独立 IP 都指向可能已不存在的地址 ——
            // 整体重置为「跟随组」，由配置侧重新挑一个当前 IP（见 set_pinned_upstream）
            reset_account_pins(&state, &config::pinned_upstream_settings());
        }
        let settings = config::pinned_upstream_settings();
        logging::log(
            "[Config]",
            &format!(
                "固定上游 IP 已更新: {} / {} 个地址（当前 {}）/ 超时 {} 秒 / 切换状态码 {:?}",
                if settings.enabled { "已开启" } else { "已关闭" },
                settings.ips.len(),
                settings.effective_current_ip().unwrap_or("无"),
                settings.wait_timeout_seconds,
                settings.status_codes.to_vec(),
            ),
        );
    }

    ok_json(pinned_json(&config::pinned_upstream_settings()))
}

/// 把该组所有账号的独立 IP 清空（回到「跟随组」）。
///
/// 只动 `workbuddy-intl` 的账号：这个功能的开关与 IP 列表本就是 provider 级的，
/// 别家账号就算历史上被写入过这个字段，也不该被另一家的配置变更牵连。
///
/// 失败只记日志、不中断保存：账号重置没做成，最坏结果是某几个账号还指着一个
/// 已删除的 IP（转发层会按它自己的值走，不影响其它账号），而保存本身是用户
/// 主动发起、必须完成的动作。
fn reset_account_pins(state: &ServerState, settings: &PinnedUpstreamSettings) {
    let store = state.store();
    // `list_accounts` 给的是公开形态（Value 数组），背后已是解密后的字段表；
    // 这里要的是「谁设了独立 IP」，所以直接读公开形态里的那一个键 —— 与界面
    // 拿到的是同一份事实，不必再走一次内部记录。
    let Some(accounts) = store.list_accounts().as_array().cloned() else {
        return;
    };
    let mut cleared = 0usize;
    for account in accounts {
        if account.get("provider").and_then(Value::as_str) != Some(PROVIDER_ID) {
            continue;
        }
        // 未设置时是 Null（见 store_view 的注入），跳过它避免每保存一次就写一遍全组
        if account.get("pinnedUpstreamIp").map_or(true, Value::is_null) {
            continue;
        }
        let Some(id) = account.get("id").and_then(Value::as_str) else {
            continue;
        };
        let patch = json!({ "pinnedUpstreamIp": Value::Null });
        match store.update_account(id, &patch) {
            Ok(_) => cleared += 1,
            Err(error) => logging::log(
                "[Config]",
                &format!("⚠️ 重置账号 {id} 的固定上游 IP 失败: {}", error.message),
            ),
        }
    }
    if cleared > 0 {
        logging::log(
            "[Config]",
            &format!(
                "固定上游 IP 列表已变更：{} 个账号的独立 IP 已重置为跟随组（当前 {}）",
                cleared,
                settings.effective_current_ip().unwrap_or("无"),
            ),
        );
    }
}

/// 响应体：GET 与 PUT 共用，键名与 config 的字段常量必然一致。
///
/// 额外带一个 `globalWaitTimeoutSeconds`：界面要显示「0 = 跟随全局（当前 N 秒）」，
/// 而全局值住在另一项配置里。由后端算好一起给，省得前端再拉一次
/// `/api/timeouts` 然后自己拼。
fn pinned_json(settings: &PinnedUpstreamSettings) -> Value {
    let global = config::timeout_settings().headers_seconds;
    json!({
        KEY_PINNED_UPSTREAM_IP: {
            FIELD_PIN_ENABLED: settings.enabled,
            FIELD_PIN_IPS: settings.ips.to_vec(),
            FIELD_PIN_WAIT_TIMEOUT_SECONDS: settings.wait_timeout_seconds,
            FIELD_PIN_STATUS_CODES: settings.status_codes.to_vec(),
            FIELD_PIN_CURRENT_IP: settings.effective_current_ip().unwrap_or(""),
        },
        // 只读的回显字段：0 = 跟随全局，界面据此渲染提示文案
        "globalWaitTimeoutSeconds": global,
        "effectiveWaitTimeoutSeconds": settings.effective_wait_timeout_seconds(global),
    })
}

/// IP 列表校验：必须是字符串数组，每项是合法 IPv4，去重后不超过上限。
///
/// 与配置侧 `pin_ips_field` 的差别（见那个函数的说明）：那里对垃圾项**丢弃**
/// （手改库的容错），这里**报错**并指出是哪一项（用户主动提交的值，要告诉他
/// 写错了）。顺序**保留** —— 列表顺序就是轮询顺序，用户排的先后有语义。
fn parse_ips(value: &Value) -> Result<Vec<String>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{FIELD_PIN_IPS} 必须是字符串数组（收到: {value}）"));
    };
    if items.len() > PIN_MAX_IPS {
        return Err(format!(
            "{FIELD_PIN_IPS} 最多 {} 项（收到 {} 项）",
            PIN_MAX_IPS,
            items.len()
        ));
    }
    let mut ips: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str().map(str::trim) else {
            return Err(format!("{FIELD_PIN_IPS} 里必须是字符串（收到: {item}）"));
        };
        if text.is_empty() {
            return Err(format!("{FIELD_PIN_IPS} 里有空白项，请删掉空行"));
        }
        if text.parse::<std::net::Ipv4Addr>().is_err() {
            return Err(format!(
                "{FIELD_PIN_IPS} 里的 {} 不是合法的 IPv4 地址（只收点分十进制，不含端口）",
                text
            ));
        }
        if !ips.iter().any(|existing| existing == text) {
            ips.push(text.to_string());
        }
    }
    Ok(ips)
}

/// 等待响应超时校验：0（跟随全局）或 `PIN_MIN..=PIN_MAX` 之间的整数秒。
fn parse_wait_timeout(value: &Value) -> Result<i64, String> {
    let number = value
        .as_i64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                .map(|raw| raw as i64)
        })
        .ok_or_else(|| {
            format!("{FIELD_PIN_WAIT_TIMEOUT_SECONDS} 必须是整数秒（收到: {value}）")
        })?;
    if number == 0 {
        return Ok(0);
    }
    if !(PIN_MIN_WAIT_TIMEOUT_SECONDS..=PIN_MAX_WAIT_TIMEOUT_SECONDS).contains(&number) {
        return Err(format!(
            "{FIELD_PIN_WAIT_TIMEOUT_SECONDS} 必须是 0（跟随全局）或 {}–{} 秒（收到: {number}）",
            PIN_MIN_WAIT_TIMEOUT_SECONDS, PIN_MAX_WAIT_TIMEOUT_SECONDS
        ));
    }
    Ok(number)
}

/// 切换状态码校验：整数数组、每项 100–599、最多 50 项、排序去重。
fn parse_status_codes(value: &Value) -> Result<Vec<u16>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!(
            "{FIELD_PIN_STATUS_CODES} 必须是状态码数组（收到: {value}）"
        ));
    };
    if items.len() > PIN_MAX_STATUS_CODES {
        return Err(format!(
            "{FIELD_PIN_STATUS_CODES} 最多 {} 项（收到 {} 项）",
            PIN_MAX_STATUS_CODES,
            items.len()
        ));
    }
    let mut codes: Vec<u16> = Vec::with_capacity(items.len());
    for item in items {
        let number = item
            .as_i64()
            .or_else(|| {
                item.as_f64()
                    .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                    .map(|raw| raw as i64)
            })
            .ok_or_else(|| format!("{FIELD_PIN_STATUS_CODES} 里必须是整数（收到: {item})"))?;
        if !(i64::from(RETRY_CODE_MIN)..=i64::from(RETRY_CODE_MAX)).contains(&number) {
            return Err(format!(
                "{FIELD_PIN_STATUS_CODES} 里的值必须是 {}-{} 的 HTTP 状态码（收到: {number}）",
                RETRY_CODE_MIN, RETRY_CODE_MAX
            ));
        }
        codes.push(number as u16);
    }
    codes.sort_unstable();
    codes.dedup();
    Ok(codes)
}
