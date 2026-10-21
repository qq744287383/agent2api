use super::*;
use crate::server::core::providers::zcode::adapter::{ZCODE_ADAPTER, ZCODE_INTL_ADAPTER};
use crate::server::core::upstream::usage::RequestTelemetry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const QUOTA_BODY: &str = r#"{"code":1005,"msg":"exceed quota limit","logid":"test-request"}"#;

async fn mock_upstream(
    status: u16,
    content_type: &'static str,
    body: &'static str,
) -> (
    TransportRequest,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let app = axum::Router::new().route(
        "/messages",
        axum::routing::post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [(axum::http::header::CONTENT_TYPE, content_type)],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/messages", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        TransportRequest {
            url,
            headers: vec![("Content-Type".into(), "application/json".into())],
            payload: r#"{"stream":true}"#.into(),
            proxy: None,
            // 测试替身按默认能力位（false = 直连），与生产里不带这一步的家同款
            system_proxy_when_unset: false,
            // 固定上游 IP 在这些用例里不参与（关着的默认值即「不固定」）；
            // 真正覆盖切换行为的用例自带构造，见本文件末尾的分组
            pin: None,
        },
        hits,
        task,
    )
}

#[tokio::test]
async fn zcode_http_200_quota_json_is_classified_before_streaming_without_resend() {
    for adapter in [&ZCODE_ADAPTER, &ZCODE_INTL_ADAPTER] {
        let (transport, hits, task) =
            mock_upstream(200, "application/json; charset=utf-8", QUOTA_BODY).await;
        let mut budget = RetryBudget::new(3);
        let capture = crate::server::core::debug_traffic::TrafficCapture::begin("quota-test");
        let result = send_with_retry(
            adapter,
            &transport,
            &mut budget,
            Some(&capture),
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("HTTP 200 quota envelope must not enter the success stream"),
        };
        assert!(matches!(
            failure.class,
            UpstreamErrorClass::QuotaLimited {
                status: 429,
                upstream_code: Some(1005),
                reset_at: None,
                ..
            }
        ));
        assert_eq!(failure.error.status_code, 429);
        assert_eq!(failure.error.upstream_code, Some(1005));
        assert!(failure.error.message.contains("exceed quota limit"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(budget.remaining, 3);
        assert_eq!(capture.captured_body(), QUOTA_BODY);
    }
}

#[test]
fn zcode_header_detection_handles_mime_parameters_without_requiring_sse_headers() {
    use axum::http::{header::CONTENT_TYPE, HeaderMap, HeaderValue};
    for (mime, expected) in [
        (None, false),
        (Some("application/json"), true),
        (Some("Application/JSON; charset=utf-8"), true),
        (Some("text/event-stream; charset=utf-8"), false),
        (Some("application/octet-stream"), false),
    ] {
        let mut headers = HeaderMap::new();
        if let Some(mime) = mime {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
        }
        assert_eq!(ZCODE_ADAPTER.is_error_response(200, &headers), expected);
        assert!(ZCODE_ADAPTER.is_error_response(401, &headers));
        assert!(ZCODE_ADAPTER.is_error_response(429, &headers));
    }
}

#[tokio::test]
async fn zcode_http_auth_and_quota_errors_keep_their_status() {
    for (status, body) in [(401, ""), (429, r#"{"msg":"rate limited"}"#)] {
        let (transport, _, task) = mock_upstream(status, "application/json", body).await;
        let result = send_with_retry(
            &ZCODE_ADAPTER,
            &transport,
            &mut RetryBudget::new(0),
            None,
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("HTTP error must not be accepted"),
        };
        assert_eq!(failure.error.status_code, i32::from(status));
        if status == 401 {
            assert!(matches!(
                failure.class,
                UpstreamErrorClass::TokenExpired { .. }
            ));
        } else {
            assert!(matches!(
                failure.class,
                UpstreamErrorClass::QuotaLimited { .. }
            ));
        }
    }
}

#[tokio::test]
async fn zcode_unknown_or_malformed_success_json_returns_an_error() {
    for body in [r#"{"code":9999,"msg":"unexpected rejection"}"#, "not json"] {
        let (transport, _, task) = mock_upstream(200, "application/json", body).await;
        let result = send_with_retry(
            &ZCODE_ADAPTER,
            &transport,
            &mut RetryBudget::new(0),
            None,
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("non-SSE ZCode response must not be accepted"),
        };
        assert_eq!(failure.error.status_code, 502);
        assert!(failure.error.message.contains(if body == "not json" {
            "not json"
        } else {
            "unexpected rejection"
        }));
    }
}

#[tokio::test]
async fn zcode_success_sse_and_other_providers_json_remain_unconsumed() {
    let sse = "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\"}}\n\n";
    let cases: [(&dyn ProviderAdapter, &str, &str); 2] = [
        (&ZCODE_ADAPTER, "text/event-stream", sse),
        (
            adapter_for(ProviderKind::WorkBuddy),
            "application/json",
            QUOTA_BODY,
        ),
    ];
    for (adapter, content_type, body) in cases {
        let (transport, _, task) = mock_upstream(200, content_type, body).await;
        let result = send_with_retry(
            adapter,
            &transport,
            &mut RetryBudget::new(0),
            None,
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let response = match result {
            Ok(response) => response,
            Err(failure) => panic!("valid response rejected: {}", failure.error.message),
        };
        assert_eq!(response.text().await.unwrap(), body);
    }
}

// ── 会话式路径的限额记账（`mark_conversation_limited`）──────────────────────
//
// 这四条钉的是同一处缺陷：改造前该分支内联调用 `rotate::mark_account_limited`，
// `reset_at` 实参写死 `None` ⇒ 适配器算好的恢复时刻被丢在函数参数上，存储层落到
// 「缺失即 10 分钟」的兜底冷却（`account_store::store_admin::mark_rate_limited`）。
// 症状不是报错，而是**同一个已耗尽的账号按兜底常量的节拍被反复重新撞**：CodeArts
// 的福利日池要到第二天零点才有额度，生产实测两小时内白撞 8 次（每次 0.2–0.3 秒、
// 被拒不计费、整条请求仍 200 —— 只看响应看不出来）。
//
// 因此在改前的代码上第 1、4 条必红；第 2、3 条是「修复没有过头」的对照组。

static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

/// 临时库目录，Drop 时删除。
///
/// 刻意不在建库前删了事：文件名带 pid，下一轮 pid 变了就永远删不到（曾在一台机器上
/// 攒出几百份）。调用方按 `let (_dir, service, id) = temp_service();` 接 —— 局部变量
/// 逆序析构，先关库再删目录。
struct TempDbDir(std::path::PathBuf);

impl Drop for TempDbDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 一份临时库 + 挂在其上的 [`UpstreamService`] + 里面唯一那条账号的 id。
///
/// 不发任何网络请求：记账与读回都只碰存储层，与转发选路用的是同一份读写口径。
fn temp_service() -> (TempDbDir, UpstreamService, String) {
    use crate::server::core::account_store::AccountStore;
    use crate::server::core::auth::AuthService;
    use crate::server::db::Db;

    let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("pl-quota-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = AccountStore::with_db(Some(Db::open(&dir.join("agent2api.db")).expect("临时库应当能建起来")));
    let saved = store
        .add_account(
            &json!({"auth": {"accessToken": "at"}, "account": {"uid": "quota-1"}}),
            None,
            None,
        )
        .expect("临时账号应当能建起来");
    let id = saved["id"].as_str().expect("公开形态应带 id").to_string();
    let service = UpstreamService::new(store.clone(), AuthService::for_store(store));
    (TempDbDir(dir), service, id)
}

/// 本家分类结果 → 记账名单（只取本次这一个键，绕开目录缓存的全局态）。
fn single(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

#[test]
fn stateful_quota_bookkeeping_keeps_the_adapter_reset_at() {
    let (_dir, service, id) = temp_service();
    let now = logging::now_ms();
    // 可辨恢复时刻：3 小时后。刻意与 10 分钟兜底不同量级 —— 改前这里落的是
    // now+600000，等值断言必红；而文案里没有任何日期形态，兜底的第二次机会
    // （`errors::parse_quota_reset_at`）也救不出这个值，差异只能来自透传。
    let reset_at = now + 3 * 60 * 60 * 1000;
    let class = UpstreamErrorClass::QuotaLimited {
        reset_at: Some(reset_at),
        message: "上游返回 403: insufficient quota".to_string(),
        upstream_code: Some(4291200),
        status: 403,
    };
    // 整组（福利池按账号记账）与单键走的是同一个循环，这里给两个名字覆盖循环体
    let group = vec!["m-benefit-a".to_string(), "m-benefit-b".to_string()];
    mark_conversation_limited(&service, &id, &group, &class, "上游返回 403: insufficient quota");
    for name in &group {
        let stored = rotate::account_limit_reset_at(&service, &id, name);
        assert_eq!(stored, reset_at, "{name} 的 resetAt 应当是适配器给的恢复时刻");
        let minutes = (stored - now) / 60_000;
        assert!(
            (stored - (now + 10 * 60 * 1000)).abs() > 60_000,
            "{name} 落到了 10 分钟兜底冷却：{minutes} 分钟"
        );
    }
    // 同一份分类里的另两个字段也照原样落地（写死 None 的那版同样丢掉过它们吗 ——
    // 没有，但这里是唯一一处能一次钉住「记账四元组一致」的地方）
    let limits = {
        let snapshot = service.store.list_accounts();
        snapshot["accounts"].as_array().unwrap()[0]["rateLimits"].clone()
    };
    assert_eq!(limits["m-benefit-a"]["status"], json!(403));
    assert_eq!(limits["m-benefit-a"]["code"], json!(4291200));
}

#[test]
fn stateful_quota_bookkeeping_falls_back_when_adapter_gives_no_reset_at() {
    let (_dir, service, id) = temp_service();
    let now = logging::now_ms();
    // 429 是分钟级限流，CodeArts 的 classify 那边就给 None（给它零点会把一个只是
    // 暂时繁忙的账号打死一整天）—— 兜底冷却是它应得的行为，修复不许把它抹掉。
    // 文案取无日期形态：解析兜底（`parse_quota_reset_at`）也救不出时刻。
    let message = "上游返回 429: too many requests";
    let class = UpstreamErrorClass::QuotaLimited {
        reset_at: None,
        message: message.to_string(),
        upstream_code: None,
        status: 429,
    };
    mark_conversation_limited(&service, &id, &single("m-429"), &class, message);
    let stored = rotate::account_limit_reset_at(&service, &id, "m-429");
    let seconds = (stored - now) / 1000;
    assert!(
        stored > now + 9 * 60 * 1000 && stored <= now + 11 * 60 * 1000,
        "无恢复时刻时应落 10 分钟兜底，实际 {seconds} 秒"
    );
}

#[test]
fn stateful_quota_bookkeeping_ignores_non_quota_classes() {
    let (_dir, service, id) = temp_service();
    // 对照组：调用方的 `if let` 已经拦下非 QuotaLimited，函数自己也不该动手 ——
    // 内容闸门/权限那一类失败本来就不该罚账号（冷却一整天尤其不能给它们）
    let message = "上游返回 403: permission denied";
    let class = UpstreamErrorClass::Fatal {
        status: 403,
        message: message.to_string(),
        upstream_code: None,
    };
    mark_conversation_limited(&service, &id, &single("m-fatal"), &class, message);
    assert_eq!(rotate::account_limit_reset_at(&service, &id, "m-fatal"), 0, "非限额分类不该落冷却");
}

#[test]
fn codearts_daily_pool_envelope_cools_to_the_boundary_instead_of_ten_minutes() {
    use crate::server::errors::GatewayError;

    let (_dir, service, id) = temp_service();
    // 生产库里那条真实文案（HTTP 200 的 SSE 里折出 403，首包门送进会话式分支）
    let error = GatewayError::with_status(403, "上游报告 InferHub.4291.200：insufficient quota");
    let class = adapter_for(ProviderKind::CodeArts).classify_conversation_error(&error);
    let UpstreamErrorClass::QuotaLimited { reset_at: Some(reset_at), .. } = &class else {
        panic!("福利池耗尽的文案应分类为带恢复时刻的 QuotaLimited，实际 {class:?}");
    };
    mark_conversation_limited(&service, &id, &single("glm-5.3-flash"), &class, &error.message);

    let stored = rotate::account_limit_reset_at(&service, &id, "glm-5.3-flash");
    assert_eq!(stored, *reset_at, "落库的恢复时刻必须是适配器算出的日池重置点");
    // ── 断言「是下一个北京零点」，而不是「距现在 > 10 分钟」（2026-10-13 修）──
    //
    // 原断言写的是 `minutes > 10`，本意是「不是 10 分钟兜底常量」；但它有个
    // 时变缺陷：日池重置点就是**下一个北京零点**，于是每天 23:50–00:00 这十分钟
    // 里「距零点」天然小于 10 分钟，用例必然变红（实测 2026-10-13 23:49 撞上）。
    // 那是测试写法的问题，不是实现的 —— 与兜底常量无关的失败被报成了兜底。
    //
    // 改成直接钉住真正的判据：恢复时刻必须落在**当天或次日的北京零点**上，
    // 且距现在不超过 24 小时（日池的语义就是「到零点重置」）。这样任何时刻
    // 跑都成立，也不再依赖「现在离零点有多远」。
    let minutes = (stored - logging::now_ms()) / 60_000;
    assert!(
        (0..=24 * 60).contains(&minutes),
        "日池的恢复时刻应当落在 24 小时以内，实际 {minutes} 分钟"
    );
    // 判据：恢复时刻在北京时区里必须是 00:00:00 —— 日池按天重置。
    // 10 分钟兜底常量（now + 10min）落在零点上的概率可忽略，这一条足以把它排除。
    let at = crate::server::core::beijing::date_time_of_ms(stored).expect("合法时间戳");
    use chrono::Timelike;
    assert!(
        at.hour() == 0 && at.minute() == 0 && at.second() == 0,
        "恢复时刻必须是北京零点（日池按天重置），实际 {}",
        at.format("%Y-%m-%d %H:%M:%S")
    );
}

#[test]
fn that_one_argument_is_the_divider_between_a_day_and_ten_minutes() {
    let (_dir, service, id) = temp_service();
    // 同一个账号库里放两条账号：一条按「改后的写法」记账，一条按「改前的写法」
    // （同一个 rotate::mark_account_limited，只是恢复时刻写死 None）。
    // 同一份分类、同一句文案 —— 差别只有那一行实参。
    let pre_fix = service
        .store
        .add_account(
            &json!({"auth": {"accessToken": "at2"}, "account": {"uid": "quota-2"}}),
            None,
            None,
        )
        .expect("对照账号应当能建起来");
    let pre_fix_id = pre_fix["id"].as_str().expect("公开形态应带 id").to_string();

    let now = logging::now_ms();
    let reset_at = now + 3 * 60 * 60 * 1000;
    let message = "上游返回 403: insufficient quota";
    let class = UpstreamErrorClass::QuotaLimited {
        reset_at: Some(reset_at),
        message: message.to_string(),
        upstream_code: Some(4291200),
        status: 403,
    };
    mark_conversation_limited(&service, &id, &single("m-after"), &class, message);
    // 改前的那一发：另三个字段照旧，恢复时刻写成 None
    rotate::mark_account_limited(&service, &pre_fix_id, "m-before", 403, Some(4291200), None, message);

    let after = rotate::account_limit_reset_at(&service, &id, "m-after");
    let before = rotate::account_limit_reset_at(&service, &pre_fix_id, "m-before");
    assert_eq!(after, reset_at, "改后应透传适配器给的恢复时刻");
    let before_minutes = (before - now) / 60_000;
    assert!(
        before_minutes >= 9 && before_minutes <= 11,
        "改前落的是存储层兜底冷却（10 分钟），实际 {before_minutes} 分钟"
    );
    // 分水岭本身：同一份分类结果，两种写法差出 2 小时以上 —— 白撞节拍就是从这里来的
    assert!(
        after - before > 2 * 60 * 60 * 1000,
        "透传与写死 None 的差距应当是「一整天 vs 10 分钟」量级，实际差 {} 分钟",
        (after - before) / 60_000
    );
}

const APIG_TOO_LARGE_BODY: &str = r#"{"error_msg":"Request entity too large","error_code":"APIG.0201","request_id":"apig-test"}"#;

fn fatal(status: u16) -> UpstreamErrorClass {
    UpstreamErrorClass::Fatal {
        status,
        message: format!("上游返回 {status}"),
        upstream_code: None,
    }
}

/// 413 说的是「这份字节太大」，重发一遍没有任何会变的量 ⇒ 不消耗原地重发预算。
/// 改造前它落到兜底那一档，三次 ×5 秒间隔 + 每次约 13 秒往返 = 客户端的 60 秒
/// 等待墙先到，面板只剩「408 客户端在响应完成前断开」，真因（413）埋在明细里。
#[test]
fn a_payload_size_rejection_gets_no_in_place_retry() {
    assert!(
        PAYLOAD_SIZE_REJECTION_STATUSES.contains(&413),
        "413 应当在「按体积拒收」名单里"
    );
    assert!(
        fallback_retry_advice(&fatal(413), 3, 413).is_none(),
        "预算还剩 3 次也不该给 413 提原地重发"
    );
}

/// 反对照：其它 4xx/未知失败照旧走兜底原地重发（这条改动只放开"体积拒收"那一格，
/// 没有把 400 这类"含义由上游决定"的码一并排除）。404 已于 issue #255 单独豁免
/// （路由 / 模型名类确定性失败，见 `DETERMINISTIC_NOT_FOUND_STATUSES`），不在此列。
#[test]
fn other_fatal_statuses_still_get_the_fallback_resend() {
    for status in [400u16, 409] {
        assert!(
            fallback_retry_advice(&fatal(status), 3, status).is_some(),
            "HTTP {status} 的语义由上游决定，兜底原地重发的既有行为不该变"
        );
    }
    assert!(
        !TRANSIENT_RETRY_STATUSES.contains(&413),
        "413 不该被当成瞬时抖动（否则瞬时那一档会把它重发回去）"
    );
}

/// 端到端：打到真实返回 413 的替身上身，**只发一次**（改造前是 1 + 3 次）。
#[tokio::test]
async fn a_real_413_response_is_sent_once_and_handed_back() {
    let (transport, hits, task) =
        mock_upstream(413, "application/json; charset=utf-8", APIG_TOO_LARGE_BODY).await;
    let mut budget = RetryBudget::new(3);
    let capture = crate::server::core::debug_traffic::TrafficCapture::begin("payload-too-large");
    let result = send_with_retry(
        &ZCODE_ADAPTER,
        &transport,
        &mut budget,
        Some(&capture),
        &RequestTelemetry::new(),
        false,
    )
    .await;
    task.abort();
    let failure = match result {
        Ok(_) => panic!("HTTP 413 不该被当成成功流"),
        Err(failure) => failure,
    };
    assert_eq!(
        1,
        hits.load(Ordering::SeqCst),
        "体积拒收的失败应当一次就交回，不该原地重发"
    );
    assert!(
        matches!(failure.class, UpstreamErrorClass::Fatal { status: 413, .. }),
        "状态码 413 要原样带回，不该被折成别的失败"
    );
}

#[test]
fn a_deterministic_404_skips_the_in_place_resend() {
    // issue #255 的形状：OfficeAce 对目录里有、套餐里没有的名字回
    // `404 {"code":null,"message":"Invalid model."}`，旧判据把它当普通 Fatal，
    // 原地重发 3 次 × 5 秒才换号——客户端白等 15 秒，换来的还是同一个 404。
    let not_found = UpstreamErrorClass::Fatal {
        status: 404,
        upstream_code: None,
        message: "上游返回 404: {\"code\":null,\"message\":\"Invalid model.\"}".to_string(),
    };
    assert!(
        fallback_retry_advice(&not_found, 3, 404).is_none(),
        "名字类确定性失败不该再原地重发"
    );
    // 对照：普通 400 仍走原地重发（400 的语义由上游定，重发是安全默认）
    let bad_request = UpstreamErrorClass::Fatal {
        status: 400,
        upstream_code: None,
        message: "上游返回 400: whatever".to_string(),
    };
    assert!(
        fallback_retry_advice(&bad_request, 3, 400).is_some(),
        "404 的豁免不许顺手把 400 也豁免了"
    );
}

// ── 固定上游 IP 的切换判据（缝隙 1 的纯判定部分）──────────────────
//
// 这一组钉的是「什么情况下才该切 IP」。判据本身是两条纯逻辑：
//   · `pin_switch_due`：要不要切（超时 / 命中用户填的状态码）；
//   · `is_wait_timeout`：这次失败算不算「等太久了」。
//
// ── 为什么必须区分「等待响应超时」与「连接超时」────────────────
// 传输层失败被统一收敛成 `Fatal{status:502}`（与 Node 的 fetchViaProxy 一致），
// 于是「上游真的回了 502」与「压根没连上」在 `class` 上长得一模一样。若不区分：
//   · 一次「连不上」会被当成「命中了用户填的 502」，多切一次 IP（IP 消耗翻倍）；
//   · 日志里写的原因还是错的（说「状态码命中」，实际是超时）。
// 这两条用例就是这层判据的唯一防线。

/// 动全局配置的用例串行跑。
///
/// `config::set_pinned_upstream` 改的是**进程级**快照，而这些用例要读它
/// （`pin_switch_due` 内部现读配置）。并行跑时一条用例刚写完、另一条就把它
/// 覆盖掉，症状是「单跑绿、全量跑红」—— 与 `config` 模块自己的 `CONFIG_LOCK`
/// 同一手法、同一理由（那边也是私有锁，两个模块各自持有即可，因为互不重叠）。
static PIN_CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hold_pin_config() -> std::sync::MutexGuard<'static, ()> {
    PIN_CONFIG_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 构造一个「传输层失败」形态的 `OutboundFailure`。
fn transport_failure(reason: &str, timed_out: bool) -> OutboundFailure {
    let message = format!("{reason}（直连）");
    OutboundFailure {
        class: UpstreamErrorClass::Fatal {
            status: 502,
            message: message.clone(),
            upstream_code: None,
        },
        error: crate::server::errors::GatewayError::with_status(502, message),
        timed_out,
        // 传输层失败：压根没拿到上游响应（这正是它与「上游真回 502」的分界）
        transport: true,
    }
}

/// 构造一个「上游真的回了错误响应」形态的 `OutboundFailure`。
fn status_failure(status: u16) -> OutboundFailure {
    let message = format!("上游返回 {status}");
    OutboundFailure {
        class: fatal(status),
        error: crate::server::errors::GatewayError::with_status(i32::from(status), message),
        // 上游回了响应 ⇒ 既不是传输层超时、也不是传输层失败
        timed_out: false,
        transport: false,
    }
}

/// 只有 `等待响应超时` 算「等太久了」；连接超时与连不上都不是。
///
/// 钉住的写歪：把判据写成 `error.is_timeout()` 或「包含『超时』二字」——
/// 那样 `连接中超时(30秒)` 也会命中，而它说明的是「这个 IP 连不上」，
/// 与用户填的「等多久算这条线路不行」是两件事。
#[test]
fn only_the_wait_for_response_timeout_counts_as_too_slow() {
    assert!(is_wait_timeout(&super::super::request::UpstreamRequestError {
        message: "等待响应超时(5秒，直连)".to_string(),
        reason: "等待响应超时(5秒)".to_string(),
    }));
    for reason in ["连接中超时(30秒)", "上游连接失败"] {
        assert!(
            !is_wait_timeout(&super::super::request::UpstreamRequestError {
                message: reason.to_string(),
                reason: reason.to_string(),
            }),
            "{reason} 说明的是连不上/没连上，不该被当成「等太久」"
        );
    }
}

/// 没钉 IP 时一律不切：功能没开启就不该有任何行为变化。
#[test]
fn no_pin_means_no_switch_no_matter_what_failed() {
    assert!(!pin_switch_due(&transport_failure("等待响应超时(5秒)", true), None));
    assert!(!pin_switch_due(&status_failure(502), None));
}

/// 超时命中就切，**不需要**用户填的状态码配合。
#[test]
fn a_wait_timeout_switches_even_when_the_status_list_is_empty() {
    let _guard = hold_pin_config();
    let pin = upstream_ip_pin::PinTarget::new("1.2.3.4").expect("测试地址应当合法");
    // 状态码名单为空（用户显式保存过空数组）：只按超时判
    let _ = config::set_pinned_upstream(config::PinnedUpstreamPatch {
        enabled: Some(true),
        ips: Some(vec!["1.2.3.4".into(), "5.6.7.8".into()]),
        status_codes: Some(vec![]),
        wait_timeout_seconds: Some(5),
        current_ip: None,
    });
    assert!(
        pin_switch_due(&transport_failure("等待响应超时(5秒)", true), Some(&pin)),
        "超时是独立判据，不该依赖状态码名单"
    );
}

/// 状态码命中就切；没命中就不切（这条是「不要见错就切」的对照组）。
#[test]
fn a_listed_status_code_switches_and_an_unlisted_one_does_not() {
    let _guard = hold_pin_config();
    let pin = upstream_ip_pin::PinTarget::new("1.2.3.4").expect("测试地址应当合法");
    let _ = config::set_pinned_upstream(config::PinnedUpstreamPatch {
        enabled: Some(true),
        ips: Some(vec!["1.2.3.4".into(), "5.6.7.8".into()]),
        status_codes: Some(vec![502, 504]),
        wait_timeout_seconds: Some(0),
        current_ip: None,
    });
    assert!(pin_switch_due(&status_failure(502), Some(&pin)), "502 在名单里");
    assert!(pin_switch_due(&status_failure(504), Some(&pin)), "504 在名单里");
    assert!(
        !pin_switch_due(&status_failure(500), Some(&pin)),
        "500 不在名单里 —— 它该继续走既有的原地重发 / 换号，而不是切 IP"
    );
}

/// **最关键的一条**：网关自己合成的 502（连不上 / 连接超时）不能被当成
/// 「命中了用户填的 502」。
///
/// 两种失败在 `class` 上完全同形（都是 `Fatal{status:502}`），唯一的区别是
/// `timed_out` 与 `error.message` 的来源。用户几乎一定会把 502 留在名单里
/// （那是默认值），所以这条判据错了，等于**每次连不上都白切一个 IP**。
#[test]
fn a_gateway_synthesised_502_is_not_mistaken_for_an_upstream_502() {
    let _guard = hold_pin_config();
    let pin = upstream_ip_pin::PinTarget::new("1.2.3.4").expect("测试地址应当合法");
    let _ = config::set_pinned_upstream(config::PinnedUpstreamPatch {
        enabled: Some(true),
        ips: Some(vec!["1.2.3.4".into(), "5.6.7.8".into()]),
        status_codes: Some(vec![502]),
        wait_timeout_seconds: Some(0),
        current_ip: None,
    });
    // 连不上：status 也是 502，但 timed_out = false，且它不是「上游返回的」
    let unreachable = transport_failure("上游连接失败", false);
    assert_eq!(
        unreachable.error.status_code, 502,
        "前提：传输层失败确实被收敛成 502（这条用例的意义就在这个巧合上）"
    );
    assert!(
        !pin_switch_due(&unreachable, Some(&pin)),
        "连不上不是「上游返回 502」，不该按状态码判据切 IP"
    );
    // 对照：上游真的回了 502 ⇒ 要切
    assert!(pin_switch_due(&status_failure(502), Some(&pin)));
}

// ── 固定上游 IP：端到端（真的打到 mock 上游上）────────────────────
//
// 上面那组钉的是纯判定；这一组钉的是**转发链真的按判定执行了**。
// 为什么必须有这一层：C1 那个缺陷（「跳过同账号原地重发」没生效）恰恰是
// 纯判定全绿、实际行为完全没变的典型 —— 判据写对了，但它被放在了
// `send_with_retry` 之后，而那里早就把请求原地重发完了。只有数「上游被打了几次」
// 才能发现这种事。

/// 一个**延迟响应**的 mock 上游：用来触发「等待响应超时」。
///
/// 与 `mock_upstream` 同一手法，只多一个 sleep —— 超时判据要的是「上游迟迟不回」，
/// 而立刻返回的替身永远走不到那条路径上。
async fn slow_mock_upstream(
    delay: std::time::Duration,
) -> (
    TransportRequest,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let app = axum::Router::new().route(
        "/messages",
        axum::routing::post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(delay).await;
                (
                    axum::http::StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    "data: {}\n\n",
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/messages", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        TransportRequest {
            url,
            headers: vec![("Content-Type".into(), "application/json".into())],
            payload: r#"{"stream":true}"#.into(),
            proxy: None,
            system_proxy_when_unset: false,
            pin: None,
        },
        hits,
        task,
    )
}

/// 把全局「等待响应超时」压到 1 秒（用例结束后不还原：进程级状态，
/// 还原反而会和并行用例抢）。
fn set_global_wait_timeout(seconds: i64) {
    let _ = config::set_timeouts(config::TimeoutPatch {
        connect_seconds: None,
        headers_seconds: Some(seconds),
        stream_idle_seconds: None,
        body_seconds: None,
    });
}

/// **本 issue 的核心承诺**：开了固定 IP 之后，等待响应超时**不再**在原账号上
/// 反复重发。
///
/// 改前（C1）：`transport_retry_advice` 对所有传输层失败都给重发建议，于是
/// 上游会被打 1 + 3 次（默认 retryCount=3），每次等满超时。客户端被挂住的时间
/// 是 4 × 超时 —— 这正是 issue 开篇写的那 20 分钟。
///
/// 这条用例断言的是**上游只被打了一次**：判据写对但位置放错（放在
/// `send_with_retry` 之后）时它必红。
#[tokio::test]
async fn a_pinned_wait_timeout_does_not_resend_on_the_same_account() {
    let _guard = hold_pin_config();
    set_global_wait_timeout(1);
    // 让重试间隔为 0，免得用例真的睡 5 秒
    let _ = config::set_retry(config::RetryPatch {
        count: Some(3),
        account_switch_count: Some(0),
        interval_seconds: Some(0),
        no_retry_codes: None,
    });

    let (mut transport, hits, task) = slow_mock_upstream(std::time::Duration::from_secs(5)).await;
    // 钉一个地址：判据要求 pin 有值才生效。这里钉的是 mock 上游自己的地址 ——
    // 钉的**值**不影响这条用例（请求本来就打到 127.0.0.1），要的是「pin 存在」
    // 这个前提条件成立
    transport.pin = Some(
        crate::server::core::upstream::upstream_ip_pin::PinTarget::new("127.0.0.1")
            .expect("回环地址应当合法"),
    );

    let mut budget = RetryBudget::new(3);
    let telemetry = RequestTelemetry::new();
    let result = send_with_retry(
        &crate::server::core::providers::workbuddy::adapter::WORKBUDDY_INTL_ADAPTER,
        &transport,
        &mut budget,
        None,
        &telemetry,
        false,
    )
    .await;

    assert!(result.is_err(), "超时应当以失败收场，交给上层去切 IP");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "开了固定 IP 时，等待响应超时**不该**在同账号上重发 —— \
         期望上游只被打 1 次，实际 {} 次（多打说明原地重发没被跳过）",
        hits.load(Ordering::SeqCst)
    );
    task.abort();
}

/// 对照组：**没开**固定 IP 时，同样的一次超时照旧走原地重发。
///
/// 这条证明上一条的「只打一次」不是把重试逻辑整个关掉了 —— 判据只对
/// 「已固定 IP」生效，存量行为一个字都没变。
#[tokio::test]
async fn an_unpinned_wait_timeout_still_resends_on_the_same_account() {
    let _guard = hold_pin_config();
    set_global_wait_timeout(1);
    let _ = config::set_retry(config::RetryPatch {
        count: Some(2),
        account_switch_count: Some(0),
        interval_seconds: Some(0),
        no_retry_codes: None,
    });

    let (transport, hits, task) = slow_mock_upstream(std::time::Duration::from_secs(5)).await;
    assert!(transport.pin.is_none(), "前提：这条用例不钉地址");

    let mut budget = RetryBudget::new(2);
    let telemetry = RequestTelemetry::new();
    let result = send_with_retry(
        &crate::server::core::providers::workbuddy::adapter::WORKBUDDY_INTL_ADAPTER,
        &transport,
        &mut budget,
        None,
        &telemetry,
        false,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "没开固定 IP 时应当照旧「首发 + 2 次重发」，实际 {} 次（存量行为不该被改动）",
        hits.load(Ordering::SeqCst)
    );
    task.abort();
}

// ── 固定上游 IP：**账号循环**这一层的端到端 ──────────────────────────
//
// 上面那两条 E2E 钉的是「一次发送里重发了几次」（`send_with_retry` 那一层）；
// 这一组再往上走一层，钉的是**只有账号循环才有的两件事**：
//
//   · 换号预算（`switches_left`）耗尽之后，客户端拿到的是**最后一次**上游错误；
//   · IP 列表转满一圈之后回到头部（列表是候选集，不是一次性消耗品）。
//
// 为什么这两条必须端到端：两者都是「编排层的记账」而不是「某一处的判据」。
// `take_switch` 单看是个三行函数，`IpCursor::next` 单看是取模 —— 真正决定
// 行为的是它们在 `attempt_queue` 里的调用位置与顺序（先问有没有下一个账号、
// 再扣预算；切 IP 与换号是同一次动作）。把这两个函数单独测一遍，等于只测了
// 「取模算得对」，测不到「预算真的拦住了第 3 个账号」「游标真的转回了第 1 个
// IP」。C1 那个缺陷就是纯判定全绿、实际行为没变的教训（见文件头）。

/// 每次被打都记一笔、并把**这是第几次**写进错误文案的 mock 上游。
///
/// 文案必须逐次不同：「预算耗尽返回最后一次错误」这条断言靠它区分
/// 「返回了最后一次」与「返回了第一次」—— 若每次都回同一句 502，
/// 两种结论在外部完全同形，用例就钉不住任何东西。
async fn counting_error_mock(
    status: u16,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        // workbuddy 的对话端点：`{endpoint}/v2/chat/completions`
        "/v2/chat/completions",
        axum::routing::post(move || {
            let nth = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                (
                    axum::http::StatusCode::from_u16(status)
                        .expect("用例里的状态码应当是合法的"),
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    format!(r#"{{"code":null,"message":"hit-{nth}"}}"#),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock 监听");
    let endpoint = format!("http://{}", listener.local_addr().expect("本地地址"));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("mock 服务") });
    (endpoint, hits, task)
}

/// 一份临时库 + `count` 个 workbuddy-intl 账号，全部指到同一个 mock 上游。
///
/// 账号级的 `endpoint` 覆盖是**生产能力**（staging / 自建反向代理用户的落点，
/// 见 `chat_completions_url`），这里借同一条路把请求指到 mock 上 —— 不是给
/// 测试专门开的后门。`expiresAt` 放到一天后：临期会触发 `ensure_access_token`
/// 的主动刷新，那条支路会真打一遍上游，把「打了几次」的计数搅乱。
fn service_with_accounts(endpoint: &str, count: usize) -> (TempDbDir, UpstreamService) {
    use crate::server::core::account_store::AccountStore;
    use crate::server::core::auth::AuthService;
    use crate::server::db::Db;

    let seq = TEMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("pl-pin-loop-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store =
        AccountStore::with_db(Some(Db::open(&dir.join("agent2api.db")).expect("临时库应当能建起来")));
    for index in 0..count {
        store
            .add_account(
                &json!({
                    "auth": {
                        "accessToken": format!("at-{index}"),
                        "expiresAt": logging::now_ms() as f64 + 86_400_000.0,
                    },
                    "account": { "uid": format!("pin-loop-{index}") },
                    "endpoint": endpoint,
                }),
                None,
                Some("workbuddy-intl"),
            )
            .expect("临时账号应当能建起来");
    }
    let service = UpstreamService::new(store.clone(), AuthService::for_store(store));
    (TempDbDir(dir), service)
}

/// 把**完整的账号循环**跑一遍：选路 → 适配器构请求 → 真打 mock → 分类 →
/// 按判据切 IP / 换号。返回结果与本次的记账快照（切换记录从后者读）。
///
/// 直接调 `attempt_queue`（而不是 `forward`）是为了跳过按模型名的候选链计算：
/// 这里要钉的是「循环怎么走」，不是「哪些家被选中」——后者有自己的用例。
async fn run_account_loop(
    service: &UpstreamService,
    body: &Value,
) -> (
    Result<ForwardOutcome, GatewayError>,
    crate::server::core::upstream::usage::TelemetrySnapshot,
) {
    let config_snapshot = config::current();
    let headers = axum::http::HeaderMap::new();
    let telemetry = Arc::new(RequestTelemetry::new());
    let ctx = ProviderContext {
        body,
        stream: false,
        client_headers: &headers,
        telemetry: &telemetry,
        sanitize_fingerprints: false,
        developer_role_compat: false,
        prompt: config_snapshot.prompt_plan(),
        key_scope: None,
        pinned_account: None,
        ignore_model_gate: false,
        proxy_override: None,
    };
    let mut slot: Option<InFlightGuard> = None;
    let mut connections = ConnectionGuard::new(service.connections());
    let result =
        attempt_queue(service, &ctx, &["workbuddy-intl"], &mut slot, &mut connections).await;
    (result, telemetry.snapshot())
}

/// 一份最小可发送的对话体（mock 不看内容，adapter 的归一会读 messages）。
fn chat_body() -> Value {
    json!({
        "model": "claude-sonnet-4-5-20250929",
        "messages": [{ "role": "user", "content": "hi" }],
        "stream": true,
    })
}

/// **换号预算耗尽后，交回客户端的是最后一次上游错误。**
///
/// 这条钉的是 `take_switch` 的**位置**：它在「问过有没有下一个账号」之后才扣。
/// 写反了（先扣再问）会让「队列里最后一个账号失败」也白吃一个额度；而把预算
/// 判断漏掉则会让第 3 个账号也被顺延 —— 用户填的「切换账号重试次数」形同虚设。
///
/// 断言用**逐次不同的文案**（`hit-1` / `hit-2`）把「最后一次」钉死：错误文案
/// 里必须是 `hit-2`，不能是 `hit-1`，也不能是网关自己拼的文案。
#[tokio::test]
async fn a_spent_switch_budget_hands_back_the_last_upstream_error() {
    let _guard = hold_pin_config();
    let _ = config::set_retry(config::RetryPatch {
        // 不做同账号原地重发：一次失败 = 一次上游往返，命中次数才读得成
        // 「试了几个账号」
        count: Some(0),
        // 只准换一次号：首发 + 1 次换号 = 2 个账号；第 3 个账号明明还在队列里
        account_switch_count: Some(1),
        interval_seconds: Some(0),
        no_retry_codes: Some(Vec::new()),
    });
    let _ = config::set_pinned_upstream(config::PinnedUpstreamPatch {
        enabled: Some(true),
        ips: Some(vec!["1.1.1.1".into(), "2.2.2.2".into()]),
        status_codes: Some(vec![502]),
        wait_timeout_seconds: Some(0),
        current_ip: Some("1.1.1.1".into()),
    });

    let (endpoint, hits, task) = counting_error_mock(502).await;
    let (_dir, service) = service_with_accounts(&endpoint, 3);
    let (result, _snapshot) = run_account_loop(&service, &chat_body()).await;
    task.abort();

    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("上游每次都回 502，这次请求不可能成功"),
    };
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "换号预算 1 ⇒ 首发 + 1 次换号，应当只打到 2 个账号，实际 {} 次",
        hits.load(Ordering::SeqCst)
    );
    assert_eq!(error.status_code, 502);
    assert!(
        error.message.contains("hit-2"),
        "预算耗尽时必须交回**最后一次**上游错误（hit-2），实际：{}",
        error.message
    );
    assert!(
        !error.message.contains("hit-1"),
        "…而且不能是第一次那句（两条文案逐次不同，正是为了能分辨这件事）：{}",
        error.message
    );
    assert_eq!(
        config::pinned_upstream_settings().current_ip,
        "2.2.2.2",
        "换了一次号 ⇒ 组当前 IP 前进一格"
    );
}

/// **组策略的 IP 游标转满一圈回到头部。**
///
/// 列表只有两个 IP、三个账号、换号预算 2：第一次切换 1.1.1.1 → 2.2.2.2，
/// 第二次必须**回到 1.1.1.1**（列表是候选集，不是一次性消耗品，见 ADR-0001
/// 决定 5）。若把 `next()` 写成「到末尾就停」，第二次切换会拿到空串 ——
/// 用户会看到一条「换成 」（空）的日志，而实际上那条线路根本没换。
///
/// 断言读的是**每轮尝试的切换记录**（`attempts_detail[].retries`，与用户在
/// 请求日志里看到的同一份数据），不是终态配置：终态的 `currentIp` 只能证明
/// 「最后停在第一个」，证明不了中间真的从第二个跳回来过。
#[tokio::test]
async fn the_group_ip_cursor_wraps_back_to_the_head_after_a_full_lap() {
    let _guard = hold_pin_config();
    let _ = config::set_retry(config::RetryPatch {
        count: Some(0),
        account_switch_count: Some(2),
        interval_seconds: Some(0),
        no_retry_codes: Some(Vec::new()),
    });
    let _ = config::set_pinned_upstream(config::PinnedUpstreamPatch {
        enabled: Some(true),
        ips: Some(vec!["1.1.1.1".into(), "2.2.2.2".into()]),
        status_codes: Some(vec![502]),
        wait_timeout_seconds: Some(0),
        current_ip: Some("1.1.1.1".into()),
    });

    let (endpoint, hits, task) = counting_error_mock(502).await;
    let (_dir, service) = service_with_accounts(&endpoint, 3);
    let (result, snapshot) = run_account_loop(&service, &chat_body()).await;
    task.abort();

    assert!(result.is_err(), "三个账号全 502，这次请求应当以失败收场");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "换号预算 2 ⇒ 三个账号各打一次，实际 {} 次",
        hits.load(Ordering::SeqCst)
    );
    let notes: Vec<String> = snapshot
        .attempts_detail
        .iter()
        .flat_map(|detail| detail.retries.iter().map(|event| event.reason.clone()))
        .filter(|reason| reason.contains("固定上游 IP 切换"))
        .collect();
    assert_eq!(notes.len(), 2, "两次换号 ⇒ 恰好两条切换记录：{notes:?}");
    assert!(
        notes[0].contains("1.1.1.1 换成 2.2.2.2"),
        "第一次切换应当落在列表的下一个（2.2.2.2）：{}",
        notes[0]
    );
    assert!(
        notes[1].contains("2.2.2.2 换成 1.1.1.1"),
        "列表只有两项，第二次切换必须回到头部（游标回环）：{}",
        notes[1]
    );
    assert_eq!(
        config::pinned_upstream_settings().current_ip,
        "1.1.1.1",
        "转满一圈后组当前 IP 回到第一个"
    );
}
