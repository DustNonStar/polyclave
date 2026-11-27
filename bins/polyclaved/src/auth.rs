use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

#[derive(Clone)]
pub struct LocalAuth {
    token: Arc<str>,
    host: String,
    web_origin: String,
}

impl LocalAuth {
    pub fn new(token: String, port: u16) -> Self {
        Self {
            token: token.into(),
            host: format!("127.0.0.1:{port}"),
            web_origin: format!("http://127.0.0.1:{port}"),
        }
    }
    fn allowed_origin(&self, origin: &str) -> bool {
        origin == self.web_origin
            || matches!(
                origin,
                "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
            )
    }
    fn matches(&self, secret: &[u8]) -> bool {
        let expected = self.token.as_bytes();
        secret.len() == expected.len()
            && secret
                .iter()
                .zip(expected)
                .fold(0, |acc, (&a, &b)| acc | (a ^ b))
                == 0
    }
}

pub async fn require(State(auth): State<LocalAuth>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if headers.get_all(header::HOST).iter().count() != 1
        || headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(auth.host.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let origin = match headers.get(header::ORIGIN) {
        Some(v)
            if headers.get_all(header::ORIGIN).iter().count() == 1
                && v.to_str().is_ok_and(|s| auth.allowed_origin(s)) =>
        {
            Some(v.clone())
        }
        Some(_) => return StatusCode::FORBIDDEN.into_response(),
        None => None,
    };
    let preflight = request.method() == Method::OPTIONS
        && origin.is_some()
        && headers
            .get("access-control-request-method")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|m| matches!(m, "GET" | "POST"));
    if !preflight {
        let key = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.as_bytes().strip_prefix(b"Bearer "));
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1
            || !key.is_some_and(|k| auth.matches(k))
        {
            let mut r = StatusCode::UNAUTHORIZED.into_response();
            cors(&mut r, origin);
            return r;
        }
    }
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    cors(&mut response, origin);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn cors(response: &mut Response, origin: Option<HeaderValue>) {
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type, Last-Event-ID"),
        );
    }
}
