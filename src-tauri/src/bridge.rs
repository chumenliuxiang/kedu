use std::{
    io::Read,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::{
    browser::{self, BrowserObservation},
    domain::{WebActivity, WebExceptionRequest, WebsiteObservation},
    storage::Storage,
};

const DEFAULT_ADDRESS: &str = "127.0.0.1:17361";

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid HTTP header")
}

fn respond(request: Request, status: u16, body: String) {
    let response = Response::from_string(body)
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Allow-Headers", "Content-Type"))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header("Cache-Control", "no-store"));
    let _ = request.respond(response);
}

fn json_error(request: Request, status: u16, message: &str) {
    respond(
        request,
        status,
        serde_json::json!({ "error": message }).to_string(),
    );
}

fn origin_allowed(request: &Request) -> bool {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Origin"))
        .map(|header| {
            let origin = header.value.as_str();
            origin.starts_with("chrome-extension://")
                || origin.starts_with("edge-extension://")
                || (std::env::var_os("KEDU_TEST_DATA_DIR").is_some()
                    && (origin.starts_with("http://127.0.0.1:")
                        || origin.starts_with("http://localhost:")))
        })
        .unwrap_or(true)
}

fn read_json<T: serde::de::DeserializeOwned>(request: &mut Request) -> Result<T, String> {
    let mut body = String::new();
    request
        .as_reader()
        .take(65_537)
        .read_to_string(&mut body)
        .map_err(|_| "无法读取请求")?;
    if body.len() > 65_536 {
        return Err("请求过大".into());
    }
    serde_json::from_str(&body).map_err(|_| "请求格式无效".into())
}

fn handle_observation(mut request: Request, storage: &Storage) {
    let observation: BrowserObservation = match read_json(&mut request) {
        Ok(value) => value,
        Err(error) => return json_error(request, 400, &error),
    };
    let client = observation.client_id.clone();
    let id = observation.observation_id.clone();
    browser::observe(observation);
    send_policy(request, storage, &client, &id, true);
}

fn handle_recording_observation(mut request: Request, storage: &Storage) {
    let observation: WebsiteObservation = match read_json(&mut request) {
        Ok(value) => value,
        Err(error) => return json_error(request, 400, &error),
    };
    match storage.observe_website(observation) {
        Ok(result) => respond(
            request,
            200,
            serde_json::to_string(&result).unwrap_or_default(),
        ),
        Err(error) => json_error(request, 400, &error),
    }
}

// Keep the already-installed extension protocol working. Website focus policies
// are deferred; this endpoint retains activity capture and daily quotas only.
fn handle_legacy_activity(mut request: Request, storage: &Storage) {
    let mut activity: WebActivity = match read_json(&mut request) {
        Ok(value) => value,
        Err(error) => return json_error(request, 400, &error),
    };
    let foreground_is_edge = crate::platform::foreground_activity(u64::MAX).is_some_and(|item| {
        item.process_path
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("msedge.exe"))
    });
    if !foreground_is_edge {
        return respond(
            request,
            200,
            serde_json::json!({"connected":true,"blocked":false,"notice":null,"focus":null})
                .to_string(),
        );
    }
    let result = (|| {
        let settings = storage.settings()?;
        activity.idle_seconds = crate::platform::idle_seconds();
        activity.idle_exempt = settings.count_media_while_idle && activity.media_active;
        activity.idle = activity.idle_seconds >= settings.idle_minutes.max(1) as u64 * 60
            && !activity.idle_exempt;
        storage.record_web_activity(&activity)?;
        Ok::<(), String>(())
    })();
    match result {
        Ok(()) => respond(
            request,
            200,
            serde_json::json!({"connected":true,"blocked":false,"notice":null,"focus":null})
                .to_string(),
        ),
        Err(error) => json_error(request, 400, &error),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseRequest {
    client_id: String,
    observation_id: String,
    #[serde(default)]
    rendered: bool,
}

fn send_policy(request: Request, storage: &Storage, client: &str, id: &str, record: bool) {
    match browser::policy(storage, client, id, record) {
        Ok(policy) => {
            let mut value = serde_json::to_value(policy).unwrap_or_default();
            value["blocked"] = serde_json::json!(false);
            value["notice"] = serde_json::Value::Null;
            value["focus"] = serde_json::Value::Null;
            respond(request, 200, value.to_string());
        }
        Err(error) => json_error(request, 500, &error),
    }
}

fn handle_watch(mut request: Request) {
    let lease: LeaseRequest = match read_json(&mut request) {
        Ok(value) => value,
        Err(error) => return json_error(request, 400, &error),
    };
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5)
        && browser::observation_current(&lease.client_id, &lease.observation_id)
    {
        thread::sleep(Duration::from_secs(1));
    }
    // This is just a wakeup lease. The extension samples the actual active tab
    // again afterwards; do not query all quota totals twice per sampling cycle.
    respond(
        request,
        200,
        serde_json::json!({ "connected": true }).to_string(),
    );
}

fn handle_ack(mut request: Request) {
    let lease: LeaseRequest = match read_json(&mut request) {
        Ok(value) => value,
        Err(error) => return json_error(request, 400, &error),
    };
    browser::acknowledge_overlay(&lease.client_id, &lease.observation_id, lease.rendered);
    respond(request, 200, serde_json::json!({ "ok": true }).to_string());
}

fn handle_exception(mut request: Request, storage: &Storage) {
    let exception = match read_json::<WebExceptionRequest>(&mut request) {
        Ok(exception) => exception,
        Err(_) => return json_error(request, 400, "放行格式无效"),
    };
    if exception.kind != "daily" && exception.kind != "focus" {
        return json_error(request, 400, "未知放行类型");
    }
    match storage.record_exception(
        exception.kind,
        exception.rule_id.to_string(),
        exception.subject_kind,
        exception.subject,
        exception.minutes,
        "Edge 紧急使用".into(),
    ) {
        Ok(()) => respond(request, 200, serde_json::json!({ "ok": true }).to_string()),
        Err(error) => json_error(request, 400, &error),
    }
}

fn focus_session_id(request: &mut Request) -> Result<i64, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Action {
        session_id: i64,
    }
    let action: Action = read_json(request)?;
    Ok(action.session_id)
}

fn handle_focus_leave(mut request: Request, storage: &Storage) {
    let id = match focus_session_id(&mut request) {
        Ok(id) => id,
        Err(error) => return json_error(request, 400, &error),
    };
    match storage.pause_focus(id) {
        Ok(focus) => match serde_json::to_string(&focus) {
            Ok(body) => respond(request, 200, body),
            Err(_) => json_error(request, 500, "无法生成响应"),
        },
        Err(error) => json_error(request, 400, &error),
    }
}

fn handle_focus_return(mut request: Request, storage: &Storage) {
    let id = match focus_session_id(&mut request) {
        Ok(id) => id,
        Err(error) => return json_error(request, 400, &error),
    };
    match storage.resume_focus(id) {
        Ok(focus) => match serde_json::to_string(&focus) {
            Ok(body) => respond(request, 200, body),
            Err(_) => json_error(request, 500, "无法生成响应"),
        },
        Err(error) => json_error(request, 400, &error),
    }
}

fn handle_test_statistics(mut request: Request, storage: &Storage) {
    if std::env::var_os("KEDU_TEST_DATA_DIR").is_none() {
        return json_error(request, 404, "接口不存在");
    }
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(request, 400, "无法读取请求");
    }
    let query = match serde_json::from_str(&body) {
        Ok(query) => query,
        Err(_) => return json_error(request, 400, "统计查询格式无效"),
    };
    match storage.query_statistics(query) {
        Ok(statistics) => match serde_json::to_string(&statistics) {
            Ok(body) => respond(request, 200, body),
            Err(_) => json_error(request, 500, "无法生成响应"),
        },
        Err(error) => json_error(request, 400, &error),
    }
}

fn handle(request: Request, storage: &Storage) {
    if !origin_allowed(&request) {
        return json_error(request, 403, "只接受本机浏览器扩展请求");
    }
    if request.method() == &Method::Options {
        return respond(request, 204, String::new());
    }
    match (request.method(), request.url()) {
        (&Method::Get, "/v3/recording/status") => match storage.website_recording_status() {
            Ok(status) => respond(request, 200, serde_json::to_string(&status).unwrap_or_default()),
            Err(error) => json_error(request, 500, &error),
        },
        (&Method::Post, "/v3/recording/observation") => handle_recording_observation(request, storage),
        (&Method::Get, "/v1/status") => respond(
            request,
            200,
            serde_json::json!({ "connected": true, "version": 2, "protection": browser::protection_status() }).to_string(),
        ),
        (&Method::Post, "/v1/activity") => handle_legacy_activity(request, storage),
        (&Method::Post, "/v2/observe") => handle_observation(request, storage),
        (&Method::Post, "/v2/watch") => handle_watch(request),
        (&Method::Post, "/v2/overlay") => handle_ack(request),
        (&Method::Post, "/v2/heartbeat") => {
            browser::heartbeat();
            respond(request, 200, serde_json::json!({ "connected": true, "protection": browser::protection_status() }).to_string())
        },
        (&Method::Post, "/v1/exception") => handle_exception(request, storage),
        (&Method::Post, "/v1/focus-leave") => handle_focus_leave(request, storage),
        (&Method::Post, "/v1/focus-return") => handle_focus_return(request, storage),
        (&Method::Post, "/v1/test/statistics") => handle_test_statistics(request, storage),
        _ => json_error(request, 404, "接口不存在"),
    }
}

pub fn start(storage: Storage) {
    thread::spawn(move || {
        let address =
            std::env::var("KEDU_TEST_BRIDGE_ADDRESS").unwrap_or_else(|_| DEFAULT_ADDRESS.into());
        let server = match Server::http(&address) {
            Ok(server) => server,
            Err(error) => {
                eprintln!("Edge bridge failed to bind {address}: {error}");
                return;
            }
        };
        static WATCHES: AtomicUsize = AtomicUsize::new(0);
        for request in server.incoming_requests() {
            // Long polls must not delay navigation, pause or emergency requests.
            if request.url() == "/v2/watch" {
                if WATCHES.fetch_add(1, Ordering::AcqRel) >= 16 {
                    WATCHES.fetch_sub(1, Ordering::AcqRel);
                    json_error(request, 429, "等待请求过多");
                    continue;
                }
                let storage = storage.clone();
                thread::spawn(move || {
                    handle(request, &storage);
                    WATCHES.fetch_sub(1, Ordering::AcqRel);
                });
            } else {
                handle(request, &storage);
            }
        }
    });
}
