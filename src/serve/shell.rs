//! The page the person opens, and the only data it is given.
//!
//! It shows what is running and frames each of them. It has no opinion about
//! what is inside a frame, and no way to form one: a Provider UI is on an
//! origin the page does not own — an opaque one, or one of the session's own —
//! so the page cannot read it and it cannot read the page.
//!
//! Where a UI sits, how big it is, whether it is folded away — all of that
//! lives here and in the person's browser. None of it is asked of a Provider,
//! and none of it is stored: the arrangement is the page's, and the page is
//! rebuilt every time it is opened.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CONTENT_TYPE, HeaderValue, LOCATION};
use axum::http::{Method, Response, StatusCode};
use axum::response::IntoResponse;

use super::{Addressing, ServeState, not_found};
use crate::{OriginDecision, UiSecurityPolicy};

const SHELL_HTML: &str = include_str!("assets/shell.html");
const SHELL_CSS: &str = include_str!("assets/shell.css");
const SHELL_JS: &str = include_str!("assets/shell.js");

/// The page loads its own two files, talks to its own listener, and frames
/// Provider UIs. `frame-src` is what admits those frames; nothing else is
/// allowed to load, and nothing may frame this page.
///
/// `{frame-src}` is the one directive the two shapes disagree about, because it
/// is the one place the page names where a session lives.
const SHELL_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; \
     style-src 'self'; connect-src 'self'; frame-src {frame-src}; img-src 'none'; \
     font-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub(super) async fn respond(
    state: &Arc<ServeState>,
    remainder: &str,
    path: &str,
    shell_authority: Option<&str>,
    request: Request,
) -> Response<Body> {
    let policy = UiSecurityPolicy::isolated()
        .with_content_security_policy(content_security_policy(&state.addressing));
    if policy.check_origin(&request) == OriginDecision::Rejected {
        return finish(StatusCode::FORBIDDEN.into_response(), &policy);
    }
    if request.method() != Method::GET {
        return finish(StatusCode::METHOD_NOT_ALLOWED.into_response(), &policy);
    }
    // The credential's URL has to end in a slash or the page's own relative
    // references resolve above it — `shell.css` would be asked for at the root,
    // where there is no credential and therefore nothing at all.
    if remainder == "/" && !path.ends_with('/') {
        let mut redirect = StatusCode::TEMPORARY_REDIRECT.into_response();
        if let Ok(location) = HeaderValue::from_str(&format!("{path}/")) {
            redirect.headers_mut().insert(LOCATION, location);
        }
        return finish(redirect, &policy);
    }
    let response = match remainder {
        "/" => asset("text/html; charset=utf-8", SHELL_HTML),
        "/shell.css" => asset("text/css; charset=utf-8", SHELL_CSS),
        "/shell.js" => asset("text/javascript; charset=utf-8", SHELL_JS),
        "/sessions" => sessions(state, shell_authority).await,
        _ => not_found(),
    };
    finish(response, &policy)
}

/// What the page is allowed to load, which is its own files and its own frames.
///
/// In the per-session shape the frames are names under this listener's host,
/// one label each — hence the wildcard — and on whatever port the page itself
/// was reached on, which this cannot know. A source with no scheme matches the
/// page's own, so a TLS terminator in front does not have to be described here.
fn content_security_policy(addressing: &Addressing) -> HeaderValue {
    let frame_src = match addressing {
        Addressing::Path => "'self'".to_owned(),
        Addressing::PerSession { base_host } => format!("*.{base_host}:*"),
    };
    HeaderValue::from_str(&SHELL_CONTENT_SECURITY_POLICY.replace("{frame-src}", &frame_src))
        .unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'"))
}

/// What is running, as the page needs to see it.
///
/// This is the whole of the page's data. There is no separate registration, no
/// lifecycle vocabulary, no capability list — the fields are the ones a live
/// session already publishes, plus the address this listener answers it on.
///
/// `isolation` is the page's, not a session's: it says which boundary is
/// keeping the frames apart, and therefore how the page has to build one. The
/// page is told rather than made to infer it from an address it should not be
/// parsing.
async fn sessions(state: &Arc<ServeState>, shell_authority: Option<&str>) -> Response<Body> {
    match state.sessions(shell_authority).await {
        Ok(sessions) => {
            let body = serde_json::json!({
                "sessions": sessions,
                "session_count": sessions.len(),
                "isolation": match state.addressing {
                    Addressing::Path => "opaque-origin",
                    Addressing::PerSession { .. } => "own-origin",
                },
            });
            match serde_json::to_vec(&body) {
                Ok(bytes) => {
                    let mut response = Response::new(Body::from(bytes));
                    response.headers_mut().insert(
                        CONTENT_TYPE,
                        HeaderValue::from_static("application/json; charset=utf-8"),
                    );
                    response
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn asset(content_type: &'static str, body: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn finish(mut response: Response<Body>, policy: &UiSecurityPolicy) -> Response<Body> {
    policy.apply_headers(response.headers_mut());
    response
}
