use tauri::http::{HeaderValue, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder, Wry};

static REQUESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(12);

fn trusted_origin(origin: Option<&str>, dev: Option<&str>, macos: bool) -> bool {
    origin.is_none_or(|origin| {
        matches!(origin, "http://tauri.localhost" | "https://tauri.localhost" | "tauri://localhost")
            || dev == Some(origin)
            // WKWebView can serialize a local custom-scheme origin as opaque.
            // The handler additionally pins the main view and its active lease.
            || macos && origin == "null"
    })
}

fn reply(status: u16, mime: &str, body: Vec<u8>, origin: Option<&str>) -> Response<Vec<u8>> {
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = response.headers_mut();
    headers.insert(
        "Content-Type",
        HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert("Cache-Control", HeaderValue::from_static("no-store"));
    headers.insert(
        "Access-Control-Allow-Methods",
        HeaderValue::from_static("GET,POST,OPTIONS"),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        HeaderValue::from_static("Content-Type,Range"),
    );
    if let Some(origin) = origin.and_then(|v| HeaderValue::from_str(v).ok()) {
        headers.insert("Access-Control-Allow-Origin", origin);
    }
    response
}

pub fn handle(
    context: UriSchemeContext<'_, Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    let path = request.uri().path().to_owned();
    let origin = request
        .headers()
        .get("Origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let dev_origin = app
        .config()
        .build
        .dev_url
        .as_ref()
        .map(|url| url.origin().ascii_serialization());
    let trusted = trusted_origin(
        origin.as_deref(),
        dev_origin.as_deref(),
        cfg!(target_os = "macos"),
    );
    if context.webview_label() != "main"
        || !trusted
        || !crate::api::web_playback::resource_path(&path)
    {
        responder.respond(reply(
            403,
            "text/plain",
            b"Playback resource denied".to_vec(),
            None,
        ));
        return;
    }
    if request.method() == "OPTIONS" {
        responder.respond(reply(204, "text/plain", Vec::new(), origin.as_deref()));
        return;
    }
    let post = request.method() == "POST";
    if !post && request.method() != "GET" || request.body().len() > 1024 * 1024 {
        responder.respond(reply(405, "text/plain", Vec::new(), origin.as_deref()));
        return;
    }
    let Ok(permit) = REQUESTS.try_acquire() else {
        responder.respond(reply(503, "text/plain", Vec::new(), origin.as_deref()));
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _permit = permit;
        let client = {
            let state = app.state::<crate::AppState>();
            let active = state.player.as_ref().and_then(|player| player.web_view());
            let valid = active.is_some_and(|view| {
                view.source
                    .playlist_path
                    .strip_suffix("/index.m3u8")
                    .is_some_and(|base| path.starts_with(&format!("{base}/")))
            });
            if !valid {
                responder.respond(reply(410, "text/plain", Vec::new(), origin.as_deref()));
                return;
            }
            state.client.clone()
        };
        match client
            .web_playback_resource(&path, post, request.into_body())
            .await
        {
            Ok((status, mime, data)) => {
                responder.respond(reply(status, &mime, data, origin.as_deref()))
            }
            Err(_) => responder.respond(reply(
                502,
                "text/plain",
                b"Playback transport unavailable".to_vec(),
                origin.as_deref(),
            )),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::trusted_origin;
    #[test]
    fn renderer_origins_exclude_remote_sites_and_accept_the_macos_local_origin() {
        assert!(trusted_origin(Some("http://tauri.localhost"), None, false));
        assert!(trusted_origin(Some("tauri://localhost"), None, true));
        assert!(trusted_origin(Some("null"), None, true));
        assert!(!trusted_origin(Some("null"), None, false));
        assert!(!trusted_origin(Some("https://evil.example"), None, true));
        assert!(!trusted_origin(
            Some("http://127.0.0.1:5182"),
            Some("http://127.0.0.1:1420"),
            false
        ));
    }
}
