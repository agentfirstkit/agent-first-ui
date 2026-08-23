//! The page the person opens, and the only data it is given.
//!
//! It shows what is running and frames each of them. It has no opinion about
//! what is inside a frame, and no way to form one: a Provider UI is on an
//! origin the page does not own — an opaque one, or one of the session's own —
//! so the page cannot read it and it cannot read the page.
//!
//! Where a UI sits, how big it is, whether it is folded away — all of that
//! lives here and in the person's browser, and none of it is asked of a
//! Provider. Almost none of it is stored either: folding and removing come back
//! on reload, because a view somebody merely rearranged is expected to. The one
//! exception is how tall a card is, which the page cannot work out for itself
//! and should not make a person say twice.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::header::{CONTENT_TYPE, HeaderValue, LOCATION};
use axum::http::{Method, Response, StatusCode};
use axum::response::IntoResponse;

use super::attention::Phase;
use super::{Addressing, ServeState, not_found};
use crate::{APP_ICON_PATH, BASE_STYLE_PATH, OriginDecision, UiSecurityPolicy};

/// The most a page may say about who it is watching.
///
/// Generous for a list of session ids and small enough that nothing is served
/// by reading further.
const MAX_ATTENTION_BODY_BYTES: usize = 64 * 1024;

pub(super) const SHELL_JS: &str = include_str!("assets/shell.js");

/// The page loads its own files — the baseline stylesheet, its own stylesheet
/// and its own script, all from here — talks to its own listener, and frames
/// Provider UIs. `frame-src` is what admits those frames; nothing else is
/// allowed to load, and nothing may frame this page.
///
/// `{frame-src}` is the one directive the two shapes disagree about, because it
/// is the one place the page names where a session lives.
const SHELL_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; \
     style-src 'self'; connect-src 'self'; frame-src {frame-src}; img-src 'self'; \
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
    // The credential's URL has to end in a slash or the page's own relative
    // references resolve above it — `shell.css` would be asked for at the root,
    // where there is no credential and therefore nothing at all.
    if request.method() == Method::GET && remainder == "/" && !path.ends_with('/') {
        let mut redirect = StatusCode::TEMPORARY_REDIRECT.into_response();
        if let Ok(location) = HeaderValue::from_str(&format!("{path}/")) {
            redirect.headers_mut().insert(LOCATION, location);
        }
        return finish(redirect, &policy);
    }
    let response = match (request.method(), remainder) {
        (&Method::GET, "/") => asset_owned(
            "text/html; charset=utf-8",
            state.shell.page().as_bytes().to_vec(),
        ),
        (&Method::GET, "/shell.css") => {
            asset_owned("text/css; charset=utf-8", state.shell.stylesheet().to_vec())
        }
        (&Method::GET, "/shell.js") => asset("text/javascript; charset=utf-8", SHELL_JS),
        (&Method::GET, rest) if rest.strip_prefix('/') == Some(APP_ICON_PATH) => asset_owned(
            "image/svg+xml",
            state.shell.app_icon().as_svg().as_bytes().to_vec(),
        ),
        // The same floor, at the same reserved name, as the sessions this page
        // frames. A card and the page inside it disagreeing about what a focus
        // ring or a warning looks like is the one place that difference is
        // visible side by side.
        (&Method::GET, rest) if rest.strip_prefix('/') == Some(BASE_STYLE_PATH) => {
            asset("text/css; charset=utf-8", crate::page_style::BASE_STYLE)
        }
        (&Method::GET, "/sessions") => sessions(state, shell_authority).await,
        // A replacement page's own static files. Every path 404s when there is
        // no replacement, so the route is answered without asking whether one
        // exists.
        (&Method::GET, rest) if rest.starts_with("/assets/") => {
            state.shell.asset(rest.trim_start_matches("/assets/"))
        }
        (&Method::POST, "/attention") => attention(state, request).await,
        (&Method::POST, rest) => match rest
            .strip_prefix("/sessions/")
            .and_then(|rest| rest.strip_suffix("/reopen"))
        {
            Some(session_id) if !session_id.contains('/') => reopen(state, session_id),
            _ => not_found(),
        },
        _ => not_found(),
    };
    finish(response, &policy)
}

/// Somebody is here, and these are the sessions they can see.
///
/// One route for both the heartbeat the page sends while it is visible and the
/// button a person presses during a warning, because they are the same claim:
/// there is a person in front of this. What the page cannot do here is bring a
/// lapsed session back — that is [`reopen`], one card at a time.
///
/// A body that is missing, oversized or malformed renews the page and nothing
/// else. The request itself is the evidence; the list is only ever a refinement
/// of it, and refusing the whole claim over a bad list would let a page bug end
/// a session somebody is watching.
async fn attention(state: &Arc<ServeState>, request: Request) -> Response<Body> {
    let mut watching = Vec::new();
    if let Ok(bytes) = to_bytes(request.into_body(), MAX_ATTENTION_BODY_BYTES).await
        && let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(sessions) = body["sessions"].as_array()
    {
        watching.extend(
            sessions
                .iter()
                .filter_map(|session| session.as_str().map(str::to_owned)),
        );
    }
    state.attend(&watching);
    StatusCode::NO_CONTENT.into_response()
}

/// Put one lapsed session back on the page.
///
/// `404` when there was no lapse to end, so a page that asks for something it
/// invented is told, rather than being told it succeeded at nothing.
fn reopen(state: &Arc<ServeState>, session_id: &str) -> Response<Body> {
    if state.reopen(session_id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        not_found()
    }
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
            let mut page = serde_json::json!({ "state": state.shell_phase().word() });
            if let Phase::Expiring { remaining } = state.shell_phase() {
                page["expires_in_ms"] = serde_json::json!(remaining.as_millis());
            }
            let body = serde_json::json!({
                "sessions": sessions,
                "session_count": sessions.len(),
                "page": page,
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
    asset_owned(content_type, body.as_bytes().to_vec())
}

fn asset_owned(content_type: &'static str, body: Vec<u8>) -> Response<Body> {
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
