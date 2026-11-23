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
/// Images on the first page fetched immediately with high priority (above the fold).
const EAGER_IMAGE_COUNT: usize = 4;

#[derive(Clone, Debug, Serialize)]
struct ImageCardView {
    id: i64,
    url: String,
    /// Thumbnail size in px. Thumbnails keep the original's aspect ratio at
    /// THUMBNAIL_WIDTH wide, so the browser can reserve the card's height
    /// before the image arrives. `None` when the original size is unknown.
    width: Option<i64>,
    height: Option<i64>,
}

/// Width thumbnails are generated at (storage::R2Client::THUMBNAIL_MAX_DIMENSION).
const THUMBNAIL_WIDTH: i64 = 480;

/// Thumbnail size for an original of `width`x`height`, scaled to
/// THUMBNAIL_WIDTH wide. Only the aspect ratio matters to the page (cards are
/// full column width); it lets the browser size the card before the image loads.
fn thumbnail_size(width: Option<i64>, height: Option<i64>) -> (Option<i64>, Option<i64>) {
    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let th = ((h as f64) * (THUMBNAIL_WIDTH as f64) / (w as f64)).round().max(1.0) as i64;
            (Some(THUMBNAIL_WIDTH), Some(th))
        }
        _ => (None, None),
    }
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
        let dashboard_key = storage::thumbnail_key(&img.s3_key_base);
        let def_filename = "image".to_string();
        let filename = img.filename.as_ref().unwrap_or(&def_filename);
        // thumbnails are always stored as webp (see storage::upload_with_thumbnail)
        let url = resolve_image_url(state, &dashboard_key, "image/webp", filename).await;

        let (width, height) = thumbnail_size(img.width, img.height);
        rendered_images.push(ImageCardView {
            id: img.id,
            url,
            width,
            height,
        });
    }

    Ok((rendered_images, has_more))
}

pub(crate) async fn get_index(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let (rendered_images, has_more) = images_for_index(&state, 1, INDEX_PAGE_SIZE).await?;
    let count = rendered_images.len();
    // first row or two load right away; the rest lazily as you scroll
    let cards_html = render_template(
        "index_cards.html",
        context!(images => rendered_images, eager_count => EAGER_IMAGE_COUNT),
    )
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
    let body = render_template("index_cards.html", context!(images => images, eager_count => 0))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_size_scales_to_thumbnail_width() {
        assert_eq!(thumbnail_size(Some(1200), Some(1800)), (Some(480), Some(720)));
        assert_eq!(thumbnail_size(Some(7016), Some(4961)), (Some(480), Some(339)));
    }

    #[test]
    fn keeps_aspect_ratio_of_small_originals() {
        assert_eq!(thumbnail_size(Some(308), Some(400)), (Some(480), Some(623)));
    }

    #[test]
    fn unknown_size_gives_none() {
        assert_eq!(thumbnail_size(None, Some(10)), (None, None));
        assert_eq!(thumbnail_size(Some(0), Some(10)), (None, None));
    }
}
