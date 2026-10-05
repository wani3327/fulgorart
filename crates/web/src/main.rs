mod api;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use base64::Engine;
use fulgorart_db::{Db, DbConfig};
use fulgorart_storage::{self as storage, R2Client, R2Config};
use minijinja::{context, AutoEscape, Environment};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

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

struct CachedPresignedUrl {
    url: String,
    expires_at: Instant,
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
struct ImageCardView {
    id: i64,
    url: String,
}

#[derive(Clone, Debug, Serialize)]
struct TagView {
    id: i64,
    name: String,
}

fn render_template(name: &str, context: impl Serialize) -> Result<String, minijinja::Error> {
    let mut environment = Environment::new();
    environment.set_auto_escape_callback(|name| {
        if name.ends_with(".html") {
            AutoEscape::Html
        } else {
            AutoEscape::None
        }
    });
    environment.add_template("index.html", include_str!("../templates/index.html"))?;
    environment.add_template(
        "index_cards.html",
        include_str!("../templates/index_cards.html"),
    )?;
    environment.add_template("image.html", include_str!("../templates/image.html"))?;
    environment.get_template(name)?.render(context)
}

#[derive(Deserialize)]
struct IndexPageQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

const IMAGE_URL_TTL_SECS: u64 = 60 * 60;
const IMAGE_URL_CACHE_REFRESH_SECS: u64 = 5;
const INDEX_PAGE_SIZE: i64 = 60;

async fn load_index_images(
    state: &AppState,
    page: i64,
    per_page: i64,
) -> Result<(Vec<ImageCardView>, bool), StatusCode> {
    let fetch_limit = per_page.saturating_add(1);
    let images = state
        .db
        .list_image_assets(page, fetch_limit)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let has_more = images.len() as i64 > per_page;
    let mut rendered_images = Vec::with_capacity(images.len().min(per_page as usize));

    for img in images.into_iter().take(per_page as usize) {
        let dashboard_key = storage::thumbnail_key(&img.s3_key_base);
        let def_filename = "image".to_string();
        let filename = img.filename.as_ref().unwrap_or(&def_filename);
        let url = resolve_image_url(state, &dashboard_key, &img.content_type, &filename).await;

        rendered_images.push(ImageCardView { id: img.id, url });
    }

    Ok((rendered_images, has_more))
}

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

async fn get_stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/style.css"),
    )
}

async fn get_index(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let (rendered_images, has_more) = load_index_images(&state, 1, INDEX_PAGE_SIZE).await?;
    let count = rendered_images.len();
    let cards_html = render_template("index_cards.html", context!(images => rendered_images))
        .map_err(|error| {
            tracing::error!(?error, "Failed to render index cards template");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Html(
        render_template(
            "index.html",
            context! {
                count,
                cards_html,
                has_more,
                next_page => 2,
                per_page => INDEX_PAGE_SIZE,
            },
        )
        .map_err(|error| {
            tracing::error!(?error, "Failed to render index template");
            StatusCode::INTERNAL_SERVER_ERROR
        })?,
    ))
}

async fn get_index_cards(
    State(state): State<AppState>,
    Query(q): Query<IndexPageQuery>,
) -> Result<Response, StatusCode> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(INDEX_PAGE_SIZE).clamp(1, 120);
    let (images, has_more) = load_index_images(&state, page, per_page).await?;
    let body =
        render_template("index_cards.html", context!(images => images)).map_err(|error| {
            tracing::error!(?error, "Failed to render index cards template");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        header::HeaderName::from_static("x-has-more"),
        HeaderValue::from_static(if has_more { "true" } else { "false" }),
    );
    Ok(response)
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
        .route("/", get(get_index))
        .route("/api/index/cards", get(get_index_cards))
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
