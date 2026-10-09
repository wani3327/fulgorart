use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
};
use fulgorart_storage::{self as storage};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::{render_template, resolve_image_url, AppState};

const INDEX_PAGE_SIZE: i64 = 60;

#[derive(Clone, Debug, Serialize)]
struct ImageCardView {
    id: i64,
    url: String,
}

#[derive(Deserialize, Default)]
pub(crate) struct IndexPageQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

/// fetch images from DB for index
async fn images_for_index(
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
        let dashboard_key = storage::original_key(&img.s3_key_base); // temporary code for test
        // let dashboard_key = storage::thumbnail_key(&img.s3_key_base);
        let def_filename = "image".to_string();
        let filename = img.filename.as_ref().unwrap_or(&def_filename);
        let url = resolve_image_url(state, &dashboard_key, &img.content_type, &filename).await;

        rendered_images.push(ImageCardView { id: img.id, url });
    }

    Ok((rendered_images, has_more))
}

pub(crate) async fn get_index(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let (rendered_images, has_more) = images_for_index(&state, 1, INDEX_PAGE_SIZE).await?;
    let count = rendered_images.len();
    let cards_html = render_template("index_cards.html", context!(images => rendered_images))
        .await
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
        .await
        .map_err(|error| {
            tracing::error!(?error, "Failed to render index template");
            StatusCode::INTERNAL_SERVER_ERROR
        })?,
    ))
}

pub(crate) async fn get_index_cards(
    State(state): State<AppState>,
    Query(q): Query<IndexPageQuery>,
) -> Result<Response, StatusCode> {
    let page = q.page.unwrap_or(1);
    let per_page = q.per_page.unwrap_or(INDEX_PAGE_SIZE).clamp(1, 120);
    let (images, has_more) = images_for_index(&state, page, per_page).await?;
    let body = render_template("index_cards.html", context!(images => images))
        .await
        .map_err(|error| {
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
