use crate::*;

/// Binds the port and serves the Kernel, with `extra` beside its routes.
pub async fn listen(startup: &Startup, state: AppState, extra: Router<AppState>, name: &str) -> Result<()> {
    let listener = bind(startup).await?;
    println!("{name} listening on {}", startup.port);
    serve(listener, state, extra).await?;
    Ok(())
}

/// Serves the Kernel, and `extra` beside its own routes, on a bound listener.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState, extra: Router<AppState>) -> std::io::Result<()> {
    axum::serve(listener, ServiceExt::<Request>::into_make_service_with_connect_info::<std::net::SocketAddr>(stack(state, extra))).await
}

/// The Kernel's whole stack. The isolation headers and the path router wrap
/// the routes, so a path address is rewritten before routing and the headers
/// cover every response, the redirects included.
/// `extra` holds the routes of whoever starts the Kernel; they answer
/// before the Kernel's catch-all and inside the same layers.
pub fn stack(state: AppState, extra: Router<AppState>) -> tower::util::BoxCloneSyncService<Request, axum::response::Response, std::convert::Infallible> {
    let routes = Router::new()
        .route("/_konae/health", get(health))
        .route("/_konae/auth/signup", post(signup))
        .route("/_konae/auth/login", post(login))
        .route("/_konae/login", get(login_page))
        .route("/_konae/fonts/{name}", get(own_page::font))
        .merge(runtime::routes())
        .merge(extra)
        .merge(google::routes())
        .merge(accounts::routes())
        .merge(http::routes())
        .merge(invitations::routes())
        .merge(enter::routes())
        .merge(mail::routes())
        .route("/_konae/{*path}", axum::routing::any(|| async { error(StatusCode::GONE, "Use authenticated MCP for platform operations.") }))
        .route("/{*path}", get(http::get_route).post(http::post_route))
        .route("/", get(serve_root))
        .layer(axum::middleware::from_fn_with_state(state.clone(), logical_route))
        .layer(axum::middleware::from_fn_with_state(state.clone(), maintenance::guard))
        .with_state(state.clone());
    let service = ServiceBuilder::new()
        .layer(axum::middleware::from_fn_with_state(state.clone(), logical_request))
        .layer(axum::middleware::from_fn_with_state(state.clone(), security::headers))
        .layer(axum::middleware::from_fn_with_state(state.clone(), hostname::redirect))
        .layer(axum::middleware::from_fn_with_state(state, path::route))
        .service(routes);
    tower::util::BoxCloneSyncService::new(service)
}

#[derive(Clone)]
pub(crate) struct Operation(pub(crate) Arc<tokio::sync::OwnedRwLockReadGuard<()>>);

async fn logical_request(State(state): State<AppState>, mut request: Request, next: axum::middleware::Next) -> Response {
    let operation = Operation(Arc::new(state.copies.operations.clone().read_owned().await));
    request.extensions_mut().insert(operation);
    next.run(request).await
}

async fn logical_route(State(state): State<AppState>, request: Request, next: axum::middleware::Next) -> Response {
    let request = if request.extensions().get::<axum::extract::MatchedPath>().is_some_and(|path| path.as_str() == "/{*path}") {
        match functions_http::dispatch(&state, request).await {
            Ok(response) => return response,
            Err(request) => request,
        }
    } else {
        request
    };
    // The legacy handlers join their SQLite work before releasing the copy gate.
    let operation = request.extensions().get::<Operation>().map(|operation| operation.0.clone());
    let task = tokio::spawn(async move {
        let _operation = operation;
        next.run(request).await
    });
    task.await.unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({"ok": true, "kernel": "konae-rust", "version": 1,
        "copies": state.copies.health(),
        "files": {"store": state.files.store.kind(), "quotaBytes": state.files.limits.quota_bytes,
            "maxFileBytes": state.files.limits.max_file_bytes},
        "mail": state.accounts.queue.as_ref().map(|queue| queue.health())}))
}

pub fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if site == "cross-site" {
            return false;
        }
    }
    match (
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        host,
    ) {
        (Some(origin), Some(host)) => url::Url::parse(origin).ok().is_some_and(|origin| {
            let expected = url::Url::parse(&format!("{}://{host}", origin.scheme())).ok();
            expected.is_some_and(|expected| origin.origin() == expected.origin())
        }),
        (Some(_), None) => false,
        (None, _) => true,
    }
}
