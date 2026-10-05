use axum::{Json, extract::{Path, Query, State}};
use fulgorart_db::{ImageAssetRow, TagRow};
use http::StatusCode;
use serde::{Serialize, Deserialize};

use crate::{AppState};

#[derive(Deserialize)]
pub struct TagFilterQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    include: Option<String>,
    exclude: Option<String>,
}

#[derive(Deserialize)]
pub struct AddTagRequest {
    tag: String,
}

#[derive(Serialize)]
pub struct ImageWithTags {
    #[serde(flatten)]
    asset: ImageAssetRow,
    tags: Vec<TagRow>,
}

pub async fn list_images(
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

pub async fn get_image(
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

pub async fn add_tag(
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

pub async fn delete_tag(
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

pub async fn list_tags(
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