use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use bytes::Bytes;
use chrono::Utc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use futures::TryStreamExt;
use tokio::sync::oneshot;
use tracing::{debug, error, info};

use crate::metrics::{MetricsStore, RequestRecord};
use crate::router::{ResolvedRoute, Router};

pub struct AppState {
    pub router: Router,
    pub client: reqwest::Client,
    pub metrics: Arc<MetricsStore>,
    pub max_body_size: usize,
}

/// Fires a oneshot signal when dropped, used to detect stream completion.
struct StreamGuard(Option<oneshot::Sender<()>>);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

fn stub_count_tokens_response() -> Response {
    let stub = serde_json::json!({"input_tokens": 0});
    let body = Body::from(serde_json::to_vec(&stub).expect("stub serialization"));
    let mut response = Response::new(body);
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn is_hop_by_hop(name: &http::header::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn build_forwarding_headers(
    original_headers: &HeaderMap,
    route: &ResolvedRoute,
    body_len: usize,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (key, value) in original_headers {
        if key == http::header::HOST || is_hop_by_hop(key) {
            continue;
        }
        if route.strip_auth && (key == http::header::AUTHORIZATION || key.as_str() == "x-api-key") {
            continue;
        }
        headers.insert(key.clone(), value.clone());
    }

    if let Some(ref api_key) = route.api_key {
        if let Ok(value) = HeaderValue::from_str(api_key) {
            headers.insert(http::header::HeaderName::from_static("x-api-key"), value);
        } else {
            tracing::warn!("api_key contains invalid header characters, skipping");
        }
    }

    if body_len > 0 {
        headers.insert(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_str(&body_len.to_string())
                .expect("content-length is valid header value"),
        );
    }

    // Strip accept-encoding so provider doesn't compress -- we need raw bytes for streaming passthrough
    headers.remove(http::header::ACCEPT_ENCODING);

    headers
}

fn rewrite_model_in_body(
    body_json: &mut Option<serde_json::Value>,
    body_bytes: Bytes,
    new_model: &str,
) -> Result<Bytes, (StatusCode, String)> {
    if let Some(json) = body_json {
        json["model"] = serde_json::Value::String(new_model.to_string());
        serde_json::to_vec(json).map(Bytes::from).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to serialize body: {e}"),
            )
        })
    } else {
        Ok(body_bytes)
    }
}

fn parse_token_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
}

fn log_outgoing_headers(headers: &HeaderMap) {
    for (key, value) in headers {
        if matches!(
            key.as_str(),
            "x-api-key" | "authorization" | "proxy-authorization" | "cookie"
        ) {
            debug!(header = %key, value = "[REDACTED]", "outgoing header");
        } else {
            debug!(header = %key, value = ?value, "outgoing header");
        }
    }
}

async fn handle_error_response(
    upstream_response: &mut reqwest::Response,
    max_body_size: usize,
    status: StatusCode,
    response_headers: HeaderMap,
    record: RequestRecord,
    metrics: &MetricsStore,
) -> Response {
    let error_bytes = read_capped_body(upstream_response, max_body_size).await;
    let error_len = error_bytes.len();

    let mut record = record;
    record.error_body = Some(format!("HTTP {status} ({error_len} bytes)"));
    metrics.record(record);

    let mut headers = response_headers;
    headers.insert(
        http::header::CONTENT_LENGTH,
        HeaderValue::from_str(&error_len.to_string())
            .expect("content-length is valid header value"),
    );
    let mut response = Response::new(Body::from(error_bytes));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

async fn read_capped_body(response: &mut reqwest::Response, max_size: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096);
    while let Ok(Some(chunk)) = response.chunk().await {
        buf.extend_from_slice(&chunk);
        if buf.len() >= max_size {
            buf.truncate(max_size);
            break;
        }
    }
    buf
}

fn stream_response(
    upstream_response: reqwest::Response,
    status: StatusCode,
    response_headers: HeaderMap,
    record_id: u64,
    header_output_tokens: u64,
    start: Instant,
    metrics: Arc<MetricsStore>,
) -> Response {
    let byte_counter = Arc::new(AtomicU64::new(0));
    let counter = byte_counter.clone();

    let (done_tx, done_rx) = oneshot::channel();
    let guard = StreamGuard(Some(done_tx));

    let stream = upstream_response
        .bytes_stream()
        .map_ok(move |chunk| {
            counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            let _hold = &guard;
            chunk
        })
        .map_err(std::io::Error::other);

    let body = Body::from_stream(stream);

    tokio::spawn(async move {
        let _ = done_rx.await;
        let total_bytes = byte_counter.load(Ordering::Relaxed);
        let estimated = if header_output_tokens > 0 {
            header_output_tokens
        } else {
            total_bytes / 4
        };
        metrics.finalize_stream(record_id, estimated, start.elapsed());
    });

    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}

fn filter_response_headers(upstream_headers: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (key, value) in upstream_headers {
        if is_hop_by_hop(key) || key == http::header::CONTENT_ENCODING {
            continue;
        }
        headers.insert(key.clone(), value.clone());
    }
    headers
}

pub async fn handle_request(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<Response, (StatusCode, String)> {
    let start = Instant::now();
    let wallclock = Utc::now();
    let (parts, body) = request.into_parts();
    let method = parts.method.clone();
    let path = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());

    let body_bytes = axum::body::to_bytes(body, state.max_body_size)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")))?;

    let body_len = body_bytes.len();

    let (mut body_json, model) = if !body_bytes.is_empty() {
        let json: serde_json::Value = serde_json::from_slice(&body_bytes)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))?;
        let model = json
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        (Some(json), model)
    } else {
        (None, String::new())
    };

    let messages = body_json
        .as_ref()
        .and_then(|j| j.get("messages"))
        .and_then(|m| m.as_array())
        .map(|v| v.as_slice());

    let route = state.router.resolve(&model, messages, &state.client).await;

    if parts.uri.path().contains("/count_tokens") && route.stub_count_tokens {
        debug!(path = %path, "returning stub count_tokens response");
        return Ok(stub_count_tokens_response());
    }

    info!(
        model = %model,
        provider = %route.provider_url,
        rewrite = ?route.model_rewrite,
        path = %path,
        estimated_tokens = body_len / 4,
        "routing request"
    );

    let final_body = if let Some(ref new_model) = route.model_rewrite {
        rewrite_model_in_body(&mut body_json, body_bytes, new_model)?
    } else {
        body_bytes
    };

    let url = format!("{}{}", route.provider_url.trim_end_matches('/'), path);
    let headers = build_forwarding_headers(&parts.headers, &route, final_body.len());

    debug!(url = %url, "forwarding to provider");
    log_outgoing_headers(&headers);
    if !final_body.is_empty() {
        debug!(body_bytes = final_body.len(), "outgoing body");
    }

    let mut upstream_response = state
        .client
        .request(method, &url)
        .headers(headers)
        .body(final_body)
        .send()
        .await
        .map_err(|e| {
            error!(url = %url, error = %e, "provider request failed");
            (
                StatusCode::BAD_GATEWAY,
                format!("provider unreachable: {e}"),
            )
        })?;

    let status = StatusCode::from_u16(upstream_response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    info!(status = %status, url = %url, "provider responded");

    let input_tokens = parse_token_header(upstream_response.headers(), "x-usage-input-tokens")
        .unwrap_or((body_len / 4) as u64);
    let output_tokens =
        parse_token_header(upstream_response.headers(), "x-usage-output-tokens").unwrap_or(0);

    let response_headers = filter_response_headers(upstream_response.headers());

    let base_record = RequestRecord {
        id: 0,
        timestamp: start,
        wallclock,
        model: model.clone(),
        provider: route.provider_name.clone(),
        routing_method: route.routing_method,
        status: status.as_u16(),
        duration: start.elapsed(),
        input_tokens,
        output_tokens,
        error_body: None,
    };

    if status.as_u16() >= 400 {
        return Ok(handle_error_response(
            &mut upstream_response,
            state.max_body_size,
            status,
            response_headers,
            base_record,
            &state.metrics,
        )
        .await);
    }

    let record_id = state.metrics.record_pending(base_record);

    Ok(stream_response(
        upstream_response,
        status,
        response_headers,
        record_id,
        output_tokens,
        start,
        state.metrics.clone(),
    ))
}
