//! Read-only loopback observer. It never registers peers, promotes a master,
//! consumes a message, or starts the coordination daemon.
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rand::RngCore;
use std::{net::Ipv4Addr, path::PathBuf, sync::Arc};
use crate::proto::{ProjectContext, Req};

#[derive(Clone)]
struct Observer {
    socket: PathBuf,
    context: ProjectContext,
    authority: String,
    capability: String,
}

fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'".parse().unwrap());
    response
}

async fn snapshot(State(observer): State<Arc<Observer>>, headers: HeaderMap) -> Response {
    if headers.get(header::HOST).and_then(|value| value.to_str().ok()) != Some(observer.authority.as_str()) {
        return secured((StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"DASHBOARD_HOST_REJECTED"}))).into_response());
    }
    let expected = format!("Bearer {}", observer.capability);
    if headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()) != Some(expected.as_str()) {
        return secured((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"DASHBOARD_CAPABILITY_REQUIRED"}))).into_response());
    }
    let result = tokio::task::spawn_blocking(move || {
        crate::client::call_with_context::<serde_json::Value>(
            &observer.socket, &Req::BoardShow, Some(observer.context.clone()),
        )
    }).await;
    match result {
        Ok(Ok(value)) => secured(Json(value).into_response()),
        Ok(Err(error)) => secured((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":error.to_string()}))).into_response()),
        Err(error) => secured((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":error.to_string()}))).into_response()),
    }
}

pub fn run(socket: PathBuf, context: ProjectContext, port: u16) -> anyhow::Result<()> {
    // Fail before exposing a URL if the exact project route is unavailable.
    let _: serde_json::Value = crate::client::call_with_context(&socket, &Req::BoardShow, Some(context.clone()))?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let authority = listener.local_addr()?.to_string();
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let capability: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let observer = Arc::new(Observer { socket, context, authority: authority.clone(), capability: capability.clone() });
        let router = Router::new()
            .route("/", get(|| async { secured(Html(include_str!("dashboard/index.html")).into_response()) }))
            .route("/app.js", get(|| async { secured(([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], include_str!("dashboard/app.js")).into_response()) }))
            .route("/app.css", get(|| async { secured(([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("dashboard/app.css")).into_response()) }))
            .route("/api/board", get(snapshot))
            .with_state(observer);
        println!("{}", serde_json::json!({ "url": format!("http://{authority}/#{capability}"), "read_only": true, "refresh_seconds": 2 }));
        axum::serve(listener, router).with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await { eprintln!("dashboard shutdown signal failed: {error}"); }
        }).await?;
        Ok(())
    })
}
