use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use tracing::{info, warn};

static ROUTE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\{"route"\s*:\s*"([^"]+)"\}"#).expect("route regex is valid"));

use crate::config::AutoRouterConfig;
use crate::router::RouteCandidate;

const TASK_INSTRUCTION: &str = "\
You are a helpful assistant designed to find the best suited route.
You are provided with route description within <routes></routes> XML tags:
<routes>

{routes}

</routes>

<conversation>

{conversation}

</conversation>
";

const FORMAT_PROMPT: &str = "\
Your task is to decide which route is best suit with user intent on the conversation \
in <conversation></conversation> XML tags.  Follow the instruction:
1. If the latest intent from user is irrelevant or user intent is full filled, \
response with other route {\"route\": \"other\"}.
2. You must analyze the route descriptions and find the best match route for user latest intent.
3. You only response the name of the route that best matches the user's request, \
use the exact name in the <routes></routes>.

Based on your analysis, provide your response in the following JSON formats \
if you decide to match any route:
{\"route\": \"route_name\"}
";

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

fn build_prompt(routes: &[RouteCandidate], messages: &[serde_json::Value]) -> String {
    let route_defs: Vec<serde_json::Value> = routes
        .iter()
        .map(|r| serde_json::json!({"name": &r.name, "description": &r.description}))
        .collect();

    let non_system: Vec<&serde_json::Value> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
        .collect();

    let prompt = TASK_INSTRUCTION
        .replace(
            "{routes}",
            &serde_json::to_string(&route_defs).unwrap_or_default(),
        )
        .replace(
            "{conversation}",
            &serde_json::to_string(&non_system).unwrap_or_default(),
        );

    format!("{prompt}{FORMAT_PROMPT}")
}

fn parse_route_name(text: &str, valid_names: &[&str]) -> Option<String> {
    // Try full JSON parse first
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim())
        && let Some(name) = v.get("route").and_then(|r| r.as_str())
    {
        if name != "other" && valid_names.contains(&name) {
            return Some(name.to_string());
        }
        return None;
    }

    // Fallback: regex extraction
    let captures = ROUTE_REGEX.captures(text)?;
    let name = captures.get(1)?.as_str();
    if name != "other" && valid_names.contains(&name) {
        Some(name.to_string())
    } else {
        None
    }
}

pub async fn classify(
    client: &reqwest::Client,
    config: &AutoRouterConfig,
    routes: &[RouteCandidate],
    messages: &[serde_json::Value],
) -> Option<String> {
    if routes.is_empty() || messages.is_empty() {
        return None;
    }

    let prompt = build_prompt(routes, messages);
    let valid_names: Vec<&str> = routes.iter().map(|r| r.name.as_str()).collect();

    info!(
        route_count = routes.len(),
        model = %config.model,
        "auto-routing request via Arch-Router"
    );

    let body = serde_json::json!({
        "model": &config.model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 64,
        "temperature": 0.0,
        "response_format": {"type": "json_object"},
    });

    let response = match client
        .post(&config.url)
        .json(&body)
        .timeout(Duration::from_millis(config.timeout_ms))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "auto-router request failed, falling through to default");
            return None;
        }
    };

    if !response.status().is_success() {
        warn!(
            status = %response.status(),
            "auto-router returned error status, falling through to default"
        );
        return None;
    }

    let chat: ChatResponse = match response.json().await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "auto-router response parse failed, falling through to default");
            return None;
        }
    };

    let Some(content) = chat
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
    else {
        warn!("auto-router returned empty choices or no content, falling through to default");
        return None;
    };
    let result = parse_route_name(content, &valid_names);

    match &result {
        Some(name) => info!(route = %name, "auto-router selected route"),
        None => {
            let truncated: String = content.chars().take(64).collect();
            warn!(
                response = %truncated,
                "auto-router returned no match, falling through to default"
            );
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<RouteCandidate> {
        vec![
            RouteCandidate {
                name: "code_gen".to_string(),
                description: "code generation".to_string(),
            },
            RouteCandidate {
                name: "summarize".to_string(),
                description: "summarization".to_string(),
            },
        ]
    }

    fn test_config(url: &str) -> AutoRouterConfig {
        AutoRouterConfig {
            enabled: true,
            url: url.to_string(),
            model: "test-model".to_string(),
            timeout_ms: 2000,
        }
    }

    fn user_messages() -> Vec<serde_json::Value> {
        vec![serde_json::json!({"role": "user", "content": "write some code"})]
    }

    /// Starts a mock server that returns a chat completions response with the given content.
    async fn start_mock_router(content: &str) -> (String, tokio::task::JoinHandle<()>) {
        start_mock_router_with_status(200, content).await
    }

    async fn start_mock_router_with_status(
        status: u16,
        content: &str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::extract::Request;
        use axum::response::Response;
        use axum::routing::any;

        let body = serde_json::json!({
            "choices": [{"message": {"content": content}}]
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let status_code = http::StatusCode::from_u16(status).unwrap();

        let app = axum::Router::new().fallback(any(move |_req: Request| {
            let body_bytes = body_bytes.clone();
            async move {
                let mut response = Response::new(axum::body::Body::from(body_bytes));
                *response.status_mut() = status_code;
                response
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/v1/chat/completions");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, handle)
    }

    #[test]
    fn parse_clean_json() {
        let names = vec!["code_gen", "summarize"];
        assert_eq!(
            parse_route_name(r#"{"route": "code_gen"}"#, &names),
            Some("code_gen".to_string())
        );
    }

    #[test]
    fn parse_other_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name(r#"{"route": "other"}"#, &names), None);
    }

    #[test]
    fn parse_unknown_name_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name(r#"{"route": "unknown"}"#, &names), None);
    }

    #[test]
    fn parse_with_preamble() {
        let names = vec!["code_gen", "summarize"];
        let text = "Based on the analysis, the best route is:\n{\"route\": \"summarize\"}";
        assert_eq!(
            parse_route_name(text, &names),
            Some("summarize".to_string())
        );
    }

    #[test]
    fn parse_garbage_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name("not json at all", &names), None);
    }

    #[test]
    fn parse_empty_returns_none() {
        let names = vec!["code_gen"];
        assert_eq!(parse_route_name("", &names), None);
    }

    #[test]
    fn build_prompt_filters_system_messages() {
        let routes = candidates();
        let messages = vec![
            serde_json::json!({"role": "system", "content": "you are helpful"}),
            serde_json::json!({"role": "user", "content": "write code"}),
        ];
        let prompt = build_prompt(&routes, &messages);
        assert!(prompt.contains("write code"));
        assert!(!prompt.contains("you are helpful"));
        assert!(prompt.contains("code_gen"));
        assert!(prompt.contains("summarize"));
    }

    #[test]
    fn build_prompt_includes_all_routes() {
        let routes = candidates();
        let messages = vec![serde_json::json!({"role": "user", "content": "hello"})];
        let prompt = build_prompt(&routes, &messages);
        assert!(prompt.contains("code generation"));
        assert!(prompt.contains("summarization"));
    }

    #[test]
    fn build_prompt_includes_conversation() {
        let routes = candidates();
        let messages = vec![
            serde_json::json!({"role": "user", "content": "fix this bug"}),
            serde_json::json!({"role": "assistant", "content": "sure"}),
            serde_json::json!({"role": "user", "content": "now optimize it"}),
        ];
        let prompt = build_prompt(&routes, &messages);
        assert!(prompt.contains("fix this bug"));
        assert!(prompt.contains("now optimize it"));
    }

    #[tokio::test]
    async fn classify_returns_matching_route() {
        let (url, _handle) = start_mock_router(r#"{"route": "code_gen"}"#).await;
        let client = reqwest::Client::new();
        let config = test_config(&url);

        let result = classify(&client, &config, &candidates(), &user_messages()).await;
        assert_eq!(result, Some("code_gen".to_string()));
    }

    #[tokio::test]
    async fn classify_returns_none_for_other() {
        let (url, _handle) = start_mock_router(r#"{"route": "other"}"#).await;
        let client = reqwest::Client::new();
        let config = test_config(&url);

        let result = classify(&client, &config, &candidates(), &user_messages()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn classify_returns_none_on_http_error() {
        let (url, _handle) = start_mock_router_with_status(500, "internal error").await;
        let client = reqwest::Client::new();
        let config = test_config(&url);

        let result = classify(&client, &config, &candidates(), &user_messages()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn classify_returns_none_on_invalid_json() {
        // Server returns 200 but body isn't a valid ChatResponse
        use axum::extract::Request;
        use axum::response::Response;
        use axum::routing::any;

        let app = axum::Router::new().fallback(any(|_req: Request| async {
            Response::new(axum::body::Body::from("not json"))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/v1/chat/completions");
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let config = test_config(&url);
        let result = classify(&client, &config, &candidates(), &user_messages()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn classify_returns_none_on_connection_refused() {
        let client = reqwest::Client::new();
        let config = test_config("http://127.0.0.1:1/v1/chat/completions");

        let result = classify(&client, &config, &candidates(), &user_messages()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn classify_returns_none_on_timeout() {
        use axum::extract::Request;
        use axum::response::Response;
        use axum::routing::any;

        let app = axum::Router::new().fallback(any(|_req: Request| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Response::new(axum::body::Body::from("too late"))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/v1/chat/completions");
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let mut config = test_config(&url);
        config.timeout_ms = 100;

        let result = classify(&client, &config, &candidates(), &user_messages()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn classify_returns_none_for_empty_routes() {
        let client = reqwest::Client::new();
        let config = test_config("http://unused");

        let result = classify(&client, &config, &[], &user_messages()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn classify_returns_none_for_empty_messages() {
        let client = reqwest::Client::new();
        let config = test_config("http://unused");

        let result = classify(&client, &config, &candidates(), &[]).await;
        assert_eq!(result, None);
    }
}
