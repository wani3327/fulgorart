mod api;
mod index;

use std::{
    collections::HashMap,
    path::Path as FsPath,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use base64::Engine;
use fulgorart_db::{Db, DbConfig};
use fulgorart_storage::{self as storage, R2Client, R2Config};
use minijinja::{context, AutoEscape, Environment, Error, ErrorKind};
use serde::Serialize;
use tokio::sync::RwLock;

const IMAGE_URL_TTL_SECS: u64 = 60 * 60;
const IMAGE_URL_CACHE_REFRESH_SECS: u64 = 5;

#[derive(Debug, Clone)]
struct WebConfig {
    password: Option<String>,
    port: u16,
}

impl WebConfig {
    fn from_env() -> Self {
        Self {
            password: std::env::var("FULGORART_PASSWORD").ok(),
            port: std::env::var("FULGORART_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3000),
        }
    }
}

#[derive(Clone)]
struct AppState {
    db: Db,
    storage: R2Client,
    config: WebConfig,
    presigned_urls: Arc<RwLock<HashMap<String, CachedPresignedUrl>>>,
}

pub struct CachedPresignedUrl {
    url: String,
    expires_at: Instant,
}

/// s3 key to working image url
async fn resolve_image_url(
    state: &AppState,
    s3_key: &str,
    content_type: &str,
    filename: &str,
) -> String {
    // checking cache
    if let Some(c) = state.presigned_urls.read().await.get(s3_key) {
        if Instant::now() <= c.expires_at {
            return c.url.clone();
        }

        // drop outdated; handle race condition
        let mut map = state.presigned_urls.write().await;
        if let Some(c) = map.get(s3_key) {
            if Instant::now() > c.expires_at {
                map.remove(s3_key);
            } else {
                return c.url.clone();
            }
        }
    }

    let ttl = Duration::from_secs(IMAGE_URL_TTL_SECS);
    match state
        .storage
        .presigned_object_url(s3_key, ttl, content_type, filename)
        .await
    {
        Ok(url) => {
            let expires_at = Instant::now()
                + ttl.saturating_sub(Duration::from_secs(IMAGE_URL_CACHE_REFRESH_SECS));
            state.presigned_urls.write().await.insert(
                s3_key.to_owned(),
                CachedPresignedUrl {
                    url: url.clone(),
                    expires_at,
                },
            );
            url
        }
        Err(error) => {
            tracing::warn!(%s3_key, ?error, "Failed to create presigned URL, falling back to object URL");
            state.storage.object_url(s3_key)
        }
    }
}

async fn check_auth(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(expected_password) = &state.config.password {
        let auth_header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok());

        match auth_header {
            Some(header) if header.starts_with("Basic ") => {
                let encoded = &header[6..];
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| StatusCode::UNAUTHORIZED)?;
                let credentials =
                    String::from_utf8(decoded).map_err(|_| StatusCode::UNAUTHORIZED)?;
                let mut parts = credentials.splitn(2, ':');
                let _user = parts.next().unwrap_or("");
                let pass = parts.next().unwrap_or("");
                if pass == expected_password {
                    return Ok(next.run(req).await);
                }
                Err(StatusCode::UNAUTHORIZED)
            }
            _ => Err(StatusCode::UNAUTHORIZED),
        }
    } else {
        Ok(next.run(req).await)
    }
}

#[derive(Clone, Debug, Serialize)]
struct TagView {
    id: i64,
    name: String,
}

async fn render_template(name: &str, context: impl Serialize) -> Result<String, minijinja::Error> {
    let path = format!("{}/templates/{}", env!("CARGO_MANIFEST_DIR"), name);
    let source = tokio::fs::read_to_string(FsPath::new(&path))
        .await
        .map_err(|error| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("failed to read {path:?}"),
            )
            .with_source(error)
        })?;

    let mut environment = Environment::new();
    environment.set_auto_escape_callback(|name| {
        if name.ends_with(".html") {
            AutoEscape::Html
        } else {
            AutoEscape::None
        }
    });
    environment.add_template(name, &source)?;
    environment.get_template(name)?.render(context)
}

async fn get_stylesheet() -> Result<impl IntoResponse, StatusCode> {
    let stylesheet =
        tokio::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/style.css"))
            .await
            .map_err(|error| {
                tracing::error!(?error, "Failed to read stylesheet");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    Ok((
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        stylesheet,
    ))
}

async fn get_image_page(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Html<String>, StatusCode> {
    let asset = state
        .db
        .get_image_asset_by_id(id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let s3_key = storage::original_key(&asset.s3_key_base);
    let def_filename = "image".to_string();
    let filename = asset.filename.as_ref().unwrap_or(&def_filename);
    let url = resolve_image_url(&state, &s3_key, &asset.content_type, filename).await;

    let tags = state.db.get_image_tags(id).await.unwrap_or_default();
    let rendered_tags = tags
        .into_iter()
        .map(|tag| TagView {
            id: tag.id,
            name: tag.name,
        })
        .collect::<Vec<_>>();

    Ok(Html(
        render_template(
            "image.html",
            context! {
                id,
                url,
                tags => rendered_tags,
            },
        )
        .await
        .map_err(|error| {
            tracing::error!(?error, "Failed to render image template");
            StatusCode::INTERNAL_SERVER_ERROR
        })?,
    ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let db_config = DbConfig::from_env();
    let config = WebConfig::from_env();
    let db = Db::connect(&db_config.path).await?;
    let storage = R2Client::new(&R2Config::from_env()).await?;

    let state = AppState {
        db,
        storage,
        config: config.clone(),
        presigned_urls: Arc::new(RwLock::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/style.css", get(get_stylesheet))
        .route("/", get(index::get_index))
        .route("/api/index/cards", get(index::get_index_cards))
        .route("/image/:id", get(get_image_page))
        .route("/api/images", get(api::list_images))
        .route("/api/images/:id", get(api::get_image))
        .route("/api/images/:id/tags", post(api::add_tag))
        .route("/api/images/:id/tags/:tag_id", delete(api::delete_tag))
        .route("/api/tags", get(api::list_tags))
        .layer(middleware::from_fn_with_state(state.clone(), check_auth))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
