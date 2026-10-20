//! 固定上游 IP：把某个上游域名的解析结果固定到用户给的地址上。
//!
//! ── 这一家为什么要这个 ──────────────────────────────────────
//! WorkBuddy 国际版的上游是 `https://www.workbuddy.ai`，DNS 结果动态。
//! 把请求固定到一组自选 IP 上之后，用户能按线路质量挑出口，并在链路变差时
//! 换一条 —— 语义与取舍见 `docs/adr/0001-pinned-upstream-ip.md`。
//!
//! ── 三个必须分开的概念 ──────────────────────────────────────
//! 本模块只做**决策**，不碰网络也不碰存储：
//!
//!   1. [`PinTarget`]：这一次请求该把哪个域名固定到哪个地址（`None` = 不固定）；
//!   2. [`IpCursor`]：组策略下「下一个该轮到谁」（轮完一圈回到头部）；
//!   3. [`resolve_pin`]：把「账号字段 + 组设置 + 代理」三者收成一个结论。
//!
//! 真正的落地在 `core::egress`（把域名钉到地址）与转发层（失败后换下一个）。
//!
//! ── 代理为什么优先 ──────────────────────────────────────────
//! 走代理时 DNS 由代理解析，钉地址对经代理的连接**没有作用**。与其让用户
//! 以为开着却实际没生效，不如明确按代理优先，并把「未生效」记进日志
//! （见 [`PinOutcome::IgnoredByProxy`]）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use std::net::IpAddr;

use crate::server::config::PinnedUpstreamSettings;

/// 固定上游 IP 只作用于这一个域名（见 ADR-0001 决定：staging 不参与）。
///
/// 写在一处而不是散在调用点：将来若要再加一个域名，改这里 + [`PinTarget`]
/// 的构造即可，不必去找「哪儿还在拼字符串」。
pub const PINNED_HOST: &str = "www.workbuddy.ai";

/// 一次请求要把哪个域名钉到哪个地址。
///
/// `host` 之所以带着：钉地址是**按 host** 生效的（`ClientBuilder::resolve`），
/// 一个客户端可能同时被多个域名复用，钉错 host 会让请求静默走回 DNS。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinTarget {
    pub host: &'static str,
    pub ip: IpAddr,
}

impl PinTarget {
    /// 由配置里的 IP 字面量构造。解析失败返回 `None`（配置侧已校验过，
    /// 这里是防御：手改库能塞进任何字符串）。
    ///
    /// **只认 IPv4**：解析成 [`IpAddr`] 而不是 [`std::net::Ipv4Addr`] 会让
    /// `::1` 也通过，而「只收 IPv4」是明文的范围决定（IPv6 要写成 `[::1]`
    /// 才不至于与端口语法混淆，先不做）。这里显式把 v6 挡掉而不是依赖上游
    /// 解析器 —— 少一处「两个校验层口径不一致」的隐患。
    pub fn new(ip_text: &str) -> Option<Self> {
        match ip_text.parse::<IpAddr>() {
            Ok(IpAddr::V4(v4)) => Some(Self { host: PINNED_HOST, ip: IpAddr::V4(v4) }),
            _ => None,
        }
    }

    /// 缓存键里用的稳定形态（见 `egress::cache_key`）。
    pub fn cache_token(&self) -> String {
        format!("{}@{}", self.host, self.ip)
    }
}

/// 一次请求的固定 IP 决策。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinOutcome {
    /// 这次请求要把域名钉到这个地址
    Pinned(PinTarget),
    /// 用户没开这个功能（或这一家不适用）
    Disabled,
    /// 开了，但账号配了出网代理 —— 代理决定 DNS，钉地址不生效。
    ///
    /// 单独一支而不是并进 `Disabled`：这一支要**记一行日志**（用户以为开着、
    /// 实际没生效，是最难自查的一类静默故障），而 `Disabled` 什么都不该说。
    IgnoredByProxy,
    /// 开了，但没有可用的 IP（列表为空，或 IP 字面量写坏）
    NoIp,
}

impl PinOutcome {
    /// 这次请求实际要钉的地址。
    pub fn target(&self) -> Option<&PinTarget> {
        match self {
            Self::Pinned(target) => Some(target),
            _ => None,
        }
    }

    /// 是否需要记一条「未生效」的日志。
    pub fn is_ignored_by_proxy(&self) -> bool {
        matches!(self, Self::IgnoredByProxy)
    }
}

/// 把「组设置 + 账号独立 IP + 是否配了代理」收成一次决策。
///
/// 参数里的 `account_pin` 是**该账号的独立 IP**（`None` = 跟随组）；
/// `has_proxy` 表示这个账号这次请求会不会走代理。代理优先的判据只在
/// `has_proxy` 为真时生效 —— 不能反过来看「有没有配代理」，因为出网代理的
/// 解析结果可能是「配了但不可用、本次回退直连」，那种情况下钉地址是有效的。
pub fn resolve_pin(
    settings: &PinnedUpstreamSettings,
    account_pin: Option<&str>,
    has_proxy: bool,
) -> PinOutcome {
    if !settings.enabled {
        return PinOutcome::Disabled;
    }
    if has_proxy {
        return PinOutcome::IgnoredByProxy;
    }
    // 账号独立 IP 优先；它不在当前列表里时**不回落组**，而是如实按它自己的
    // 值走 —— 自动轮换刚把账号改到某个 IP、用户随即改了列表但还没保存时，
    // 这段时间内按账号自己的值更贴近「它上一次实际在用的那条线」。
    // （保存配置时写侧会把所有账号的独立 IP 清掉，所以悬空值活不过一次保存。）
    let chosen = match account_pin.map(str::trim).filter(|text| !text.is_empty()) {
        Some(ip) => Some(ip),
        None => settings.effective_current_ip(),
    };
    match chosen.and_then(PinTarget::new) {
        Some(target) => PinOutcome::Pinned(target),
        None => PinOutcome::NoIp,
    }
}

/// 组策略下的 IP 游标：从组当前 IP 之后开始，轮完一圈回到头部。
///
/// ── 为什么从「之后」而不是「当前」开始 ──────────────────────
/// 游标只在**失败后**前进：当前 IP 已经失败，下一次当然要试它后面那个。
/// 若从当前值本身开始，第一次 `next()` 会返回刚失败的那个，等于原地再来一次。
///
/// ── 为什么会「轮完一圈回到头部」 ────────────────────────────
/// IP 列表不是一次性消耗品：它只是「一次请求里可以轮着试的候选集」。
/// 唯一的预算是全局换号次数（见 ADR-0001 决定 5），所以转满一圈后从头继续。
#[derive(Clone, Debug)]
pub struct IpCursor {
    ips: Vec<String>,
    /// 下一个要返回的下标
    next_index: usize,
    /// 已经绕了几圈（仅用于日志/断言，不参与判定）
    laps: usize,
}

impl IpCursor {
    /// 以某个 IP 为起点建游标。`current` 不在列表里时从头部开始。
    pub fn new(ips: &[String], current: Option<&str>) -> Self {
        let next_index = match current.and_then(|ip| ips.iter().position(|item| item == ip)) {
            Some(index) => (index + 1) % ips.len().max(1),
            None => 0,
        };
        Self { ips: ips.to_vec(), next_index, laps: 0 }
    }

    /// 前进一个 IP。列表为空时返回 `None`。
    ///
    /// 注意每次调用都会前进一步 —— 调用方负责只在**该切换时**调它。
    pub fn next(&mut self) -> Option<&str> {
        if self.ips.is_empty() {
            return None;
        }
        let index = self.next_index % self.ips.len();
        self.next_index = (index + 1) % self.ips.len();
        if self.next_index == 0 {
            self.laps = self.laps.saturating_add(1);
        }
        self.ips.get(index).map(String::as_str)
    }

    /// 已经完整绕过几圈。
    pub fn laps(&self) -> usize {
        self.laps
    }

    /// 列表长度（为空表示没有候选）。
    pub fn len(&self) -> usize {
        self.ips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ips.is_empty()
    }
}

#[cfg(test)]
mod tests {
    //! 三个纯函数的口径：策略取值（账号优先 / 开关关闭 / 代理优先）、
    //! 游标轮转（含空列表与悬空当前值）、以及 IP 字面量的防御性解析。
    //!
    //! 这些用例都不碰网络也不碰存储 —— 它们的职责就是把「决策」钉死在
    //! 一处，让转发层里剩下的只有「按决策执行」。

    use super::*;
    use std::sync::Arc;

    fn settings(ips: &[&str], current: &str, enabled: bool) -> PinnedUpstreamSettings {
        PinnedUpstreamSettings {
            enabled,
            ips: Arc::from(ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>()),
            wait_timeout_seconds: 0,
            status_codes: Arc::from([502u16]),
            current_ip: current.to_string(),
        }
    }

    fn ips_of(text: &str) -> PinTarget {
        PinTarget::new(text).expect("测试用的地址应当合法")
    }

    #[test]
    fn account_pin_wins_over_group_current_ip() {
        let config = settings(&["1.1.1.1", "2.2.2.2"], "1.1.1.1", true);
        let outcome = resolve_pin(&config, Some("2.2.2.2"), false);
        assert_eq!(outcome.target(), Some(&ips_of("2.2.2.2")));
    }

    #[test]
    fn no_account_pin_falls_back_to_the_group_current_ip() {
        let config = settings(&["1.1.1.1", "2.2.2.2"], "2.2.2.2", true);
        let outcome = resolve_pin(&config, None, false);
        assert_eq!(outcome.target(), Some(&ips_of("2.2.2.2")));
    }

    #[test]
    fn a_blank_account_pin_counts_as_follow_group() {
        let config = settings(&["1.1.1.1"], "1.1.1.1", true);
        // 空串与纯空白都等于「没填」—— 界面清空之后提交的就是这两种之一
        assert_eq!(resolve_pin(&config, Some(""), false).target(), Some(&ips_of("1.1.1.1")));
        assert_eq!(resolve_pin(&config, Some("   "), false).target(), Some(&ips_of("1.1.1.1")));
    }

    #[test]
    fn the_switch_off_means_no_pin_at_all() {
        let config = settings(&["1.1.1.1"], "1.1.1.1", false);
        assert_eq!(resolve_pin(&config, Some("1.1.1.1"), false), PinOutcome::Disabled);
        // 关掉时连「代理未生效」都不该说 —— 用户没开这个功能，提它只会让人困惑
        assert_eq!(resolve_pin(&config, None, true), PinOutcome::Disabled);
    }

    #[test]
    fn a_proxy_beats_the_pin_and_says_so() {
        let config = settings(&["1.1.1.1"], "1.1.1.1", true);
        let outcome = resolve_pin(&config, None, true);
        assert_eq!(outcome, PinOutcome::IgnoredByProxy);
        assert!(outcome.is_ignored_by_proxy());
        assert_eq!(outcome.target(), None);
    }

    #[test]
    fn an_empty_ip_list_degrades_to_no_pin_instead_of_pinning_something() {
        let config = settings(&[], "", true);
        assert_eq!(resolve_pin(&config, None, false), PinOutcome::NoIp);
    }

    #[test]
    fn a_dangling_current_ip_falls_back_to_the_first_listed_one() {
        // 悬空值来自手改库：读侧解析已经兜过一次，这里再钉一次「决策层也不认它」
        let config = settings(&["1.1.1.1", "2.2.2.2"], "9.9.9.9", true);
        assert_eq!(resolve_pin(&config, None, false).target(), Some(&ips_of("1.1.1.1")));
    }

    #[test]
    fn a_corrupt_ip_literal_is_reported_as_no_ip_not_as_a_panic() {
        // `PinTarget::new` 是配置侧校验之后的防御层：手改库能塞进任何字符串
        assert_eq!(PinTarget::new("not-an-ip"), None);
        assert_eq!(PinTarget::new("1.2.3.999"), None);
        assert_eq!(PinTarget::new("::1"), None, "只收 IPv4");
        assert!(PinTarget::new("1.2.3.4").is_some());
    }

    #[test]
    fn the_cursor_starts_after_the_current_ip_and_wraps_to_the_front() {
        let ips = vec!["1.1.1.1".to_string(), "2.2.2.2".to_string(), "3.3.3.3".to_string()];
        let mut cursor = IpCursor::new(&ips, Some("1.1.1.1"));
        assert_eq!(cursor.next(), Some("2.2.2.2"));
        assert_eq!(cursor.next(), Some("3.3.3.3"));
        // 转满一圈回到头部，而不是停下 —— 唯一预算是换号次数（见 ADR-0001）
        assert_eq!(cursor.next(), Some("1.1.1.1"));
        assert_eq!(cursor.laps(), 1);
    }

    #[test]
    fn the_cursor_follows_a_trailing_current_ip_back_to_the_first_entry() {
        let ips = vec!["1.1.1.1".to_string(), "2.2.2.2".to_string()];
        let mut cursor = IpCursor::new(&ips, Some("2.2.2.2"));
        assert_eq!(cursor.next(), Some("1.1.1.1"));
    }

    #[test]
    fn a_dangling_cursor_start_begins_at_the_head() {
        let ips = vec!["1.1.1.1".to_string(), "2.2.2.2".to_string()];
        let mut cursor = IpCursor::new(&ips, Some("9.9.9.9"));
        assert_eq!(cursor.next(), Some("1.1.1.1"));
    }

    #[test]
    fn an_empty_cursor_never_yields_an_ip() {
        let mut cursor = IpCursor::new(&[], None);
        assert!(cursor.is_empty());
        assert_eq!(cursor.next(), None);
        assert_eq!(cursor.next(), None);
    }
}

// ─── 与存储 / 配置的接缝（本模块唯一读全局状态的地方）─────────────────

/// 这一次请求该钉到哪个地址（**只看配置，不看具体是哪一家**）。
///
/// 调用方：对话链路在编排层算好塞进 `TransportRequest`；保活 / 目录 / 余额 /
/// 续期这几条旁支直接在构造请求时调它。所有这些路径都打同一个上游域名，
/// 所以判据只有「账号自己的 IP」与「是否走代理」两个变量。
///
/// `account_pin` 由调用方从账号记录里取（`StoredAccount::pinned_upstream_ip`）。
pub fn resolve_for_account(account_pin: Option<&str>, has_proxy: bool) -> PinOutcome {
    let settings = crate::server::config::pinned_upstream_settings();
    resolve_pin(&settings, account_pin, has_proxy)
}

/// 由会话 JSON 算出固定上游 IP。
///
/// 会话形态（`store.session_for_account` / `get_session_by_id` 的产物）是转发
/// 层与各旁支共用的那一份，账号级字段都挂在它上面。账本里的独立 IP 会在
/// `session_from_record` 时透传进来 —— 若某个调用方拿到的 session 没带这个
/// 字段，它就等于「跟随组」，这是安全的缺省。
pub fn pin_for_session(session: &serde_json::Value, has_proxy: bool) -> Option<PinTarget> {
    let account_pin = session
        .get("pinnedUpstreamIp")
        .and_then(serde_json::Value::as_str);
    resolve_for_account(account_pin, has_proxy).target().cloned()
}

/// 由账号公开形态算出固定上游 IP（转发层手上是 `RouteTarget::account`）。
pub fn pin_for_account_value(account: Option<&serde_json::Value>, has_proxy: bool) -> Option<PinTarget> {
    let account_pin = account
        .and_then(|account| account.get("pinnedUpstreamIp"))
        .and_then(serde_json::Value::as_str);
    resolve_for_account(account_pin, has_proxy).target().cloned()
}
