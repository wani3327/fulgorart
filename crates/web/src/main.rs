use axum::{
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{Html, Json, Response},
    routing::{delete, get, post},
    Router,
};
use base64::Engine;
use fulgorart_db::{Db, DbConfig, ImageAssetRow, TagRow};
use fulgorart_storage::{R2Client, R2Config};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
struct WebConfig {
    password: Option<String>,
    port: u16,
}

impl WebConfig {
    fn from_env() -> Self {
        dotenvy::dotenv().ok();
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

#[derive(Deserialize)]
struct TagFilterQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    include: Option<String>,
    exclude: Option<String>,
}

#[derive(Deserialize)]
struct AddTagRequest {
    tag: String,
}

#[derive(Serialize)]
struct ImageWithTags {
    #[serde(flatten)]
    asset: ImageAssetRow,
    tags: Vec<TagRow>,
}

async fn get_index(State(state): State<AppState>) -> Html<String> {
    let images = state.db.list_image_assets(1, 50).await.unwrap_or_default();
    let mut cards = String::new();
    for img in &images {
        let url = state.storage.object_url(&img.s3_key);
        cards.push_str(&format!(
            r#"<div class="card">
  <a href="/image/{id}"><img src="{url}" loading="lazy" alt="image {id}"/></a>
</div>"#,
            id = img.id,
            url = url,
        ));
    }
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8"/>
<meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>FulgorArt</title>
<style>
body {{ font-family: sans-serif; margin: 0; background: #111; color: #eee; }}
h1 {{ padding: 1rem; }}
.grid {{ display: flex; flex-wrap: wrap; gap: 8px; padding: 1rem; }}
.card {{ width: 200px; height: 200px; overflow: hidden; background: #222; }}
.card img {{ width: 100%; height: 100%; object-fit: cover; }}
a {{ color: #aef; }}
</style>
</head>
<body>
<h1>FulgorArt ({count} images)</h1>
<div class="grid">{cards}</div>
</body>
</html>"#,
        count = images.len(),
        cards = cards,
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

    let tags = state.db.get_image_tags(id).await.unwrap_or_default();
    let url = state.storage.object_url(&asset.s3_key);
    let tag_list = tags
        .iter()
        .map(|tag| {
            format!(
                r#"<span class="tag" data-id="{}">{} <button onclick="removeTag({},{})">×</button></span>"#,
                tag.id, tag.name, id, tag.id
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    Ok(Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8"/>
<title>Image {id}</title>
<style>
body {{ font-family: sans-serif; background: #111; color: #eee; margin: 0; padding: 1rem; }}
img {{ max-width: 100%; max-height: 80vh; }}
.tag {{ background: #333; padding: 4px 8px; border-radius: 4px; margin: 2px; display: inline-block; }}
button {{ background: none; border: none; color: #f88; cursor: pointer; }}
input {{ background: #222; color: #eee; border: 1px solid #444; padding: 4px; }}
a {{ color: #aef; }}
</style>
</head>
<body>
<a href="/">← Back</a>
<h2>Image #{id}</h2>
<p><a href="{url}" target="_blank">{url}</a></p>
<img src="{url}" alt="image {id}"/>
<h3>Tags</h3>
<div id="tags">{tag_list}</div>
<div>
  <input id="newtag" placeholder="add tag..." list="taglist"/>
  <button onclick="addTag({id})">Add</button>
</div>
<script>
async function addTag(imageId) {{
  const tag = document.getElementById('newtag').value.trim();
  if (!tag) return;
  await fetch('/api/images/' + imageId + '/tags', {{
    method: 'POST',
    headers: {{'Content-Type': 'application/json'}},
    body: JSON.stringify({{tag}})
  }});
  location.reload();
}}
async function removeTag(imageId, tagId) {{
  await fetch('/api/images/' + imageId + '/tags/' + tagId, {{method: 'DELETE'}});
  location.reload();
}}
</script>
</body>
</html>"#,
        id = id,
        url = url,
        tag_list = tag_list,
    )))
}

async fn api_list_images(
    State(state): State<AppState>,
    Query(q): Query<TagFilterQuery>,
) -> Result<Json<Vec<ImageWithTags>>, StatusCode> {
    let page = q.page.unwrap_or(1);
    let per_page = q.per_page.unwrap_or(20).min(100);
    let include: Vec<String> = q
        .include
        .as_deref()
        .map(|value| value.split(',').map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    let exclude: Vec<String> = q
        .exclude
        .as_deref()
        .map(|value| value.split(',').map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();

    let assets = state
        .db
        .list_image_assets_by_tags(&include, &exclude, page, per_page)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut result = Vec::new();
    for asset in assets {
        let tags = state.db.get_image_tags(asset.id).await.unwrap_or_default();
        result.push(ImageWithTags { asset, tags });
    }
    Ok(Json(result))
}

async fn api_get_image(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ImageWithTags>, StatusCode> {
    let asset = state
        .db
        .get_image_asset_by_id(id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let tags = state.db.get_image_tags(id).await.unwrap_or_default();
    Ok(Json(ImageWithTags { asset, tags }))
}

async fn api_add_tag(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<AddTagRequest>,
) -> Result<StatusCode, StatusCode> {
    let tag = state
        .db
        .get_or_create_tag(&body.tag, None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    state
        .db
        .insert_image_tag(id, tag.id, "manual", None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::CREATED)
}

async fn api_delete_tag(
    State(state): State<AppState>,
    Path((image_id, tag_id)): Path<(i64, i64)>,
) -> Result<StatusCode, StatusCode> {
    state
        .db
        .delete_image_tag(image_id, tag_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn api_list_tags(
    State(state): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<TagRow>>, StatusCode> {
    let tags = if let Some(search) = q.get("q") {
        state
            .db
            .search_tags(search)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        state
            .db
            .list_all_tags()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(tags))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    };
    let app = Router::new()
        .route("/", get(get_index))
        .route("/image/:id", get(get_image_page))
        .route("/api/images", get(api_list_images))
        .route("/api/images/:id", get(api_get_image))
        .route("/api/images/:id/tags", post(api_add_tag))
        .route("/api/images/:id/tags/:tag_id", delete(api_delete_tag))
        .route("/api/tags", get(api_list_tags))
        .layer(middleware::from_fn_with_state(state.clone(), check_auth))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
