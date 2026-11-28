use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use polyclave_store::Store;
use std::sync::Arc;

#[derive(Clone)]
pub struct LocalAuth {
    token: Arc<str>,
    host: String,
    web_origin: String,
    store: Option<Store>,
}

impl LocalAuth {
    #[cfg(test)]
    pub fn new(token: String, port: u16) -> Self {
        Self::with_store(token, port, None)
    }

    pub fn with_store(token: String, port: u16, store: Option<Store>) -> Self {
        Self {
            token: token.into(),
            host: format!("127.0.0.1:{port}"),
            web_origin: format!("http://127.0.0.1:{port}"),
            store,
        }
    }
    /// Whether a browser may call the daemon with this `Origin`.
    ///
    /// The desktop page and the Tauri shell are accepted by name. A phone or
    /// tablet reaches the daemon through Tailscale Serve, which terminates TLS
    /// and sets the identity header; then that page's own origin is accepted too,
    /// but only when the origin is `https` and names exactly the `Host` the
    /// browser used, so a cross-site page is still refused.
    pub fn origin_allowed(&self, origin: &str, host: Option<&str>, peer: Option<&str>) -> bool {
        if origin == self.web_origin
            || matches!(
                origin,
                "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
            )
        {
            return true;
        }
        match (peer, host) {
            (Some(peer), Some(host)) => !peer.is_empty() && same_https_origin(origin, host),
            _ => false,
        }
    }
    /// The loopback `Host` value the desktop page uses.
    pub fn loopback_host(&self) -> &str {
        &self.host
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
    // Only the identity Tailscale Serve sets. A Mesh node header is a routing
    // label a caller can write itself, so it is never an identity.
    let peer_identity = headers
        .get("tailscale-user-login")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // Exactly one Host header, or none is trusted: a duplicated or missing Host
    // cannot prove which origin the browser reached.
    let host = if headers.get_all(header::HOST).iter().count() == 1 {
        headers.get(header::HOST).and_then(|v| v.to_str().ok())
    } else {
        None
    };
    let loopback_host = host == Some(auth.host.as_str());
    if !loopback_host && (peer_identity.is_none() || auth.store.is_none()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let origin = match headers.get(header::ORIGIN) {
        Some(v)
            if headers.get_all(header::ORIGIN).iter().count() == 1
                && v.to_str()
                    .is_ok_and(|s| auth.origin_allowed(s, host, peer_identity.as_deref())) =>
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
            .is_some_and(|m| matches!(m, "GET" | "POST" | "DELETE"));
    if !preflight {
        let key = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.as_bytes().strip_prefix(b"Bearer "));
        let single_auth = headers.get_all(header::AUTHORIZATION).iter().count() == 1;
        let mut authorized = false;
        if single_auth {
            if let Some(k) = key {
                if let Some(peer) = peer_identity.as_deref() {
                    // Non-loopback / peer-tagged requests require BOTH valid device token AND matching peer identity.
                    if let (Some(store), Ok(token_str)) =
                        (auth.store.clone(), std::str::from_utf8(k))
                    {
                        let token_owned = token_str.to_owned();
                        let peer_owned = peer.to_owned();
                        authorized = tokio::task::spawn_blocking(move || {
                            store
                                .verify_device_token(&token_owned, Some(&peer_owned))
                                .ok()
                                .flatten()
                                .is_some()
                        })
                        .await
                        .unwrap_or(false);
                    }
                } else if auth.matches(k) {
                    authorized = true;
                } else if let (Some(store), Ok(token_str)) =
                    (auth.store.clone(), std::str::from_utf8(k))
                {
                    let token_owned = token_str.to_owned();
                    authorized = tokio::task::spawn_blocking(move || {
                        store
                            .verify_device_token(&token_owned, None)
                            .ok()
                            .flatten()
                            .is_some()
                    })
                    .await
                    .unwrap_or(false);
                }
            }
        }
        if !authorized {
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

/// `origin` is `https://<authority>` and `<authority>` is exactly `host`
/// (case-insensitively, as host names are). Anything carrying userinfo, a path,
/// a query or a fragment is not an origin a browser sends, so it is refused.
fn same_https_origin(origin: &str, host: &str) -> bool {
    let Some(authority) = origin.strip_prefix("https://") else {
        return false;
    };
    !authority.is_empty()
        && !authority.contains(['/', '@', '?', '#', '\\'])
        && authority.eq_ignore_ascii_case(host)
}

fn cors(response: &mut Response, origin: Option<HeaderValue>) {
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, DELETE"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type, Last-Event-ID"),
        );
    }
}
