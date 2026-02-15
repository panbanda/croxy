use std::sync::Arc;
use std::time::Duration;

use axum::Router as AxumRouter;
use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use axum::routing::any;
use figment::Figment;
use figment::providers::{Format, Toml};
use http::HeaderValue;
use tokio::net::TcpListener;

use croxy::config::Config;
use croxy::metrics::MetricsStore;
use croxy::proxy::{AppState, handle_request};
use croxy::router::Router;

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Starts a mock backend that echoes request details back as JSON.
async fn start_echo_backend() -> (String, AbortOnDrop) {
    let app = AxumRouter::new().fallback(any(echo_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, AbortOnDrop(handle))
}

async fn echo_handler(request: Request) -> Response {
    let method = request.method().to_string();
    let path = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();

    let mut headers_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (key, value) in request.headers() {
        headers_map.insert(key.to_string(), value.to_str().unwrap_or("").to_string());
    }

    let body_bytes = axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024)
        .await
        .unwrap();

    let body_json: Option<serde_json::Value> = if !body_bytes.is_empty() {
        serde_json::from_slice(&body_bytes).ok()
    } else {
        None
    };

    let echo = serde_json::json!({
        "echo_method": method,
        "echo_path": path,
        "echo_headers": headers_map,
        "echo_body": body_json,
    });

    let body = Body::from(serde_json::to_vec(&echo).unwrap());
    let mut response = Response::new(body);
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// Starts a mock backend that returns an error with the given status and body size.
async fn start_error_backend(status: u16, body_size: usize) -> (String, AbortOnDrop) {
    let app = AxumRouter::new().fallback(any(move |_req: Request| async move {
        let body = vec![b'x'; body_size];
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = http::StatusCode::from_u16(status).unwrap();
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain"),
        );
        response
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, AbortOnDrop(handle))
}

/// Starts croxy with the given TOML config. Returns (proxy_url, state, abort_handle).
async fn start_proxy(config_toml: &str) -> (String, Arc<AppState>, AbortOnDrop) {
    let config: Config = Figment::new()
        .merge(Toml::string(config_toml))
        .extract()
        .unwrap();

    let router = Router::from_config(&config).unwrap();

    let state = Arc::new(AppState {
        router,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        metrics: Arc::new(MetricsStore::new(Duration::from_secs(1800))),
        max_body_size: config.server.max_body_size,
    });

    let app = AxumRouter::new()
        .fallback(any(handle_request))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, state, AbortOnDrop(handle))
}

fn make_config(backend_a_url: &str, backend_b_url: &str) -> String {
    format!(
        r#"
        [server]
        [backends.anthropic]
        url = "{backend_a_url}"
        [backends.ollama]
        url = "{backend_b_url}"
        strip_auth = true
        api_key = "ollama"
        stub_count_tokens = true
        [[routes]]
        pattern = "opus"
        backend = "anthropic"
        [[routes]]
        pattern = "sonnet|haiku"
        backend = "ollama"
        model = "qwen3-coder:30b"
        [default]
        backend = "anthropic"
        "#
    )
}

fn single_backend_config(backend_url: &str) -> String {
    single_backend_config_with(backend_url, "")
}

fn single_backend_config_with(backend_url: &str, extra_server: &str) -> String {
    format!(
        r#"
        [server]
        {extra_server}
        [backends.a]
        url = "{backend_url}"
        [[routes]]
        pattern = ".*"
        backend = "a"
        [default]
        backend = "a"
        "#
    )
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Test fixture: two echo backends + proxy with standard config. Returns handles that auto-cleanup.
struct DualBackendFixture {
    proxy_url: String,
    state: Arc<AppState>,
    _handles: (AbortOnDrop, AbortOnDrop, AbortOnDrop),
}

impl DualBackendFixture {
    async fn new() -> Self {
        let (anthropic_url, h1) = start_echo_backend().await;
        let (ollama_url, h2) = start_echo_backend().await;
        let (proxy_url, state, h3) = start_proxy(&make_config(&anthropic_url, &ollama_url)).await;
        Self {
            proxy_url,
            state,
            _handles: (h1, h2, h3),
        }
    }

    async fn post_messages(&self, model: &str) -> serde_json::Value {
        self.post_messages_with_headers(model, &[]).await
    }

    async fn post_messages_with_headers(
        &self,
        model: &str,
        extra_headers: &[(&str, &str)],
    ) -> serde_json::Value {
        let mut req = client()
            .post(format!("{}/v1/messages", self.proxy_url))
            .header("content-type", "application/json")
            .header("x-api-key", "sk-real-key");
        for &(key, value) in extra_headers {
            req = req.header(key, value);
        }
        req.json(&serde_json::json!({"model": model, "messages": []}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn routes_opus_to_anthropic_backend() {
    let f = DualBackendFixture::new().await;
    let resp = f.post_messages("claude-opus-4-6").await;

    assert!(
        resp["echo_path"].as_str().unwrap().contains("/v1/messages"),
        "path should be forwarded"
    );
    assert_eq!(
        resp["echo_body"]["model"].as_str().unwrap(),
        "claude-opus-4-6"
    );
}

#[tokio::test]
async fn routes_sonnet_to_ollama_with_model_rewrite() {
    let f = DualBackendFixture::new().await;
    let resp = f.post_messages("claude-sonnet-4-5-20250929").await;

    assert_eq!(
        resp["echo_body"]["model"].as_str().unwrap(),
        "qwen3-coder:30b"
    );
}

#[tokio::test]
async fn strips_auth_headers_for_ollama() {
    let f = DualBackendFixture::new().await;
    let resp = f
        .post_messages_with_headers(
            "claude-sonnet-4-5-20250929",
            &[("authorization", "Bearer sk-real-key")],
        )
        .await;

    let headers = &resp["echo_headers"];
    assert_eq!(headers.get("authorization"), None);
    assert_eq!(headers["x-api-key"].as_str().unwrap(), "ollama");
}

#[tokio::test]
async fn preserves_auth_headers_for_anthropic() {
    let f = DualBackendFixture::new().await;
    let resp = f.post_messages("claude-opus-4-6").await;

    assert_eq!(
        resp["echo_headers"]["x-api-key"].as_str().unwrap(),
        "sk-real-key"
    );
}

#[tokio::test]
async fn stubs_count_tokens_for_ollama_route() {
    let f = DualBackendFixture::new().await;

    let resp: serde_json::Value = client()
        .post(format!("{}/v1/messages/count_tokens", f.proxy_url))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"model": "claude-sonnet-4-5-20250929", "messages": []}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp["input_tokens"].as_i64().unwrap(), 0);
    assert!(resp.get("echo_method").is_none());
}

#[tokio::test]
async fn forwards_count_tokens_for_anthropic_route() {
    let f = DualBackendFixture::new().await;

    let resp: serde_json::Value = client()
        .post(format!("{}/v1/messages/count_tokens", f.proxy_url))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-real-key")
        .json(&serde_json::json!({"model": "claude-opus-4-6", "messages": []}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(resp.get("echo_method").is_some());
}

#[tokio::test]
async fn strips_accept_encoding() {
    let f = DualBackendFixture::new().await;
    let resp = f
        .post_messages_with_headers("claude-opus-4-6", &[("accept-encoding", "gzip, deflate")])
        .await;

    assert!(
        resp["echo_headers"].get("accept-encoding").is_none(),
        "accept-encoding should be stripped"
    );
}

#[tokio::test]
async fn records_metrics_for_proxied_request() {
    let f = DualBackendFixture::new().await;

    client()
        .post(format!("{}/v1/messages", f.proxy_url))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"model": "claude-opus-4-6", "messages": []}))
        .send()
        .await
        .unwrap();

    let snap = f.state.metrics.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].model, "claude-opus-4-6");
    assert_eq!(snap[0].backend, "anthropic");
    assert_eq!(snap[0].status, 200);
    assert!(snap[0].duration.as_nanos() > 0);
    assert!(snap[0].input_tokens > 0);
    assert!(snap[0].error_body.is_none());
}

#[tokio::test]
async fn returns_502_when_backend_unreachable() {
    let (proxy_url, _state, _h) = start_proxy(&single_backend_config("http://127.0.0.1:1")).await;

    let resp = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"model": "anything", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 502);
}

#[tokio::test]
async fn returns_400_for_invalid_json_body() {
    let (backend_url, _h1) = start_echo_backend().await;
    let (proxy_url, _state, _h2) = start_proxy(&single_backend_config(&backend_url)).await;

    let resp = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn rejects_oversized_request_body() {
    let (backend_url, _h1) = start_echo_backend().await;
    let (proxy_url, _state, _h2) = start_proxy(&single_backend_config_with(
        &backend_url,
        "max_body_size = 256",
    ))
    .await;

    let large_body = serde_json::json!({
        "model": "test",
        "messages": [{"content": "x".repeat(512)}]
    });

    let resp = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("content-type", "application/json")
        .json(&large_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn accepts_body_within_configured_limit() {
    let (backend_url, _h1) = start_echo_backend().await;
    let (proxy_url, _state, _h2) = start_proxy(&single_backend_config_with(
        &backend_url,
        "max_body_size = 10485760",
    ))
    .await;

    let resp = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"model": "test", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn caps_error_response_body() {
    let (error_url, _h1) = start_error_backend(500, 65536).await;
    let (proxy_url, state, _h2) = start_proxy(&single_backend_config_with(
        &error_url,
        "max_body_size = 4096",
    ))
    .await;

    let resp = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"model": "test", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 500);

    let body = resp.bytes().await.unwrap();
    assert!(
        body.len() <= 4096,
        "error body should be capped, got {} bytes",
        body.len()
    );

    let snap = state.metrics.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].status, 500);
    assert!(snap[0].error_body.is_some());
    assert!(snap[0].error_body.as_ref().unwrap().len() <= 1024);
}

#[tokio::test]
async fn records_error_metrics_for_backend_errors() {
    let (error_url, _h1) = start_error_backend(429, 32).await;
    let (proxy_url, state, _h2) = start_proxy(&single_backend_config(&error_url)).await;

    let resp = client()
        .post(format!("{proxy_url}/v1/messages"))
        .header("content-type", "application/json")
        .json(&serde_json::json!({"model": "test-model", "messages": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 429);

    let snap = state.metrics.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].status, 429);
    assert_eq!(snap[0].model, "test-model");
    assert!(snap[0].error_body.is_some());
}

#[tokio::test]
async fn get_request_without_body_routes_to_default() {
    let (backend_url, _h1) = start_echo_backend().await;
    let (proxy_url, _state, _h2) = start_proxy(&single_backend_config(&backend_url)).await;

    let resp: serde_json::Value = client()
        .get(format!("{proxy_url}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp["echo_method"].as_str().unwrap(), "GET");
    assert!(resp["echo_path"].as_str().unwrap().contains("/v1/models"));
}
