use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
};
use fulgorart_storage::{self as storage};
use minijinja::context;
use serde::{Deserialize, Serialize};

use fulgorart_db::ImageAssetRow;

use crate::{render_template, resolve_image_url, AppState};

const INDEX_PAGE_SIZE: i64 = 60;
/// Images on the first page fetched immediately with high priority (above the fold).
const EAGER_IMAGE_COUNT: usize = 4;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImageCardView {
    id: i64,
    url: String,
    /// Thumbnail size in px. Thumbnails keep the original's aspect ratio at
    /// THUMBNAIL_WIDTH wide, so the browser can reserve the card's height
    /// before the image arrives. `None` when the original size is unknown.
    width: Option<i64>,
    height: Option<i64>,
    post_id: Option<i64>,
    /// Images folded into this card (same post, matching the search); >1 shows
    /// a count badge and unfolds on click.
    group_size: i64,
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
    /// Tag search, see `parse_tag_query`.
    q: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct GalleryQuery {
    q: Option<String>,
}

/// Most tags one search may use (each included tag is a join).
const MAX_SEARCH_TAGS: usize = 12;

/// Booru-style tag search: space-separated tags must all be present;
/// `-tag` excludes images that have it. Duplicates are dropped.
fn parse_tag_query(query: &str) -> (Vec<String>, Vec<String>) {
    let (mut include, mut exclude) = (Vec::<String>::new(), Vec::<String>::new());
    for token in query.split_whitespace().take(MAX_SEARCH_TAGS) {
        let (list, tag) = match token.strip_prefix('-') {
            Some(tag) if !tag.is_empty() => (&mut exclude, tag),
            Some(_) => continue, // a lone "-"
            None => (&mut include, token),
        };
        if !list.iter().any(|t| t == tag) {
            list.push(tag.to_owned());
        }
    }
    (include, exclude)
}

/// fetch images from DB for index
/// Card for one image (thumbnail URL + size), `group_size` images folded into it.
pub(crate) async fn card_view(state: &AppState, img: &ImageAssetRow, group_size: i64) -> ImageCardView {
    let thumbnail = storage::thumbnail_key(&img.s3_key_base);
    let filename = img.filename.as_deref().unwrap_or("image");
    // thumbnails are always stored as webp (see storage::upload_with_thumbnail)
    let url = resolve_image_url(state, &thumbnail, "image/webp", filename).await;
    let (width, height) = thumbnail_size(img.width, img.height);
    ImageCardView {
        id: img.id,
        url,
        width,
        height,
        post_id: img.post_id,
        group_size,
    }
}

/// One gallery page, folded by post (see `Db::list_image_groups`).
async fn images_for_index(
    state: &AppState,
    page: i64,
    per_page: i64,
    query: &str,
) -> Result<(Vec<ImageCardView>, bool), StatusCode> {
    // one extra row tells whether there's a next page
    let fetch_limit = per_page.saturating_add(1);
    let offset = (page.max(1) - 1).saturating_mul(per_page);
    let (include, exclude) = parse_tag_query(query);
    let groups = state
        .db
        .list_image_groups(&include, &exclude, fetch_limit, offset)
        .await
        .map_err(|error| {
            tracing::error!(?error, "Failed to list images");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let has_more = groups.len() as i64 > per_page;
    let mut cards = Vec::with_capacity(groups.len().min(per_page as usize));
    for group in groups.into_iter().take(per_page as usize) {
        cards.push(card_view(state, &group.asset, group.group_size).await);
    }
    Ok((cards, has_more))
}

pub(crate) async fn get_index(
    State(state): State<AppState>,
    Query(gallery): Query<GalleryQuery>,
) -> Result<Html<String>, StatusCode> {
    let query = gallery.q.as_deref().unwrap_or("").trim().to_owned();
    let (rendered_images, has_more) = images_for_index(&state, 1, INDEX_PAGE_SIZE, &query).await?;
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
                q => query,
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
    let query = q.q.as_deref().unwrap_or("").trim();
    let (images, has_more) = images_for_index(&state, page, per_page, query).await?;
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

/// Cards for every image of a post that matches the search; the gallery
/// swaps a folded card for these when it's clicked.
pub(crate) async fn get_post_cards(
    State(state): State<AppState>,
    Path(post_id): Path<i64>,
    Query(gallery): Query<GalleryQuery>,
) -> Result<Html<String>, StatusCode> {
    let (include, exclude) = parse_tag_query(gallery.q.as_deref().unwrap_or(""));
    let images = state
        .db
        .list_post_images(post_id, &include, &exclude)
        .await
        .map_err(|error| {
            tracing::error!(?error, post_id, "Failed to list post images");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let mut cards = Vec::with_capacity(images.len());
    for img in &images {
        cards.push(card_view(&state, img, 1).await);
    }
    let body = render_template("index_cards.html", context!(images => cards, eager_count => 0))
        .await
        .map_err(|error| {
            tracing::error!(?error, "Failed to render post cards");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Html(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(v: &[&str]) -> Vec<String> {
        v.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn parses_include_and_exclude_tags() {
        assert_eq!(
            parse_tag_query("  1girl pink_hair  -explicit "),
            (tags(&["1girl", "pink_hair"]), tags(&["explicit"]))
        );
        assert_eq!(parse_tag_query(""), (vec![], vec![]));
        // a lone "-" is ignored, duplicates dropped, names kept verbatim
        assert_eq!(
            parse_tag_query("- 1girl 1girl -x -x :o"),
            (tags(&["1girl", ":o"]), tags(&["x"]))
        );
    }

    #[test]
    fn caps_number_of_search_tags() {
        let many = (0..30).map(|i| format!("t{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(parse_tag_query(&many).0.len(), MAX_SEARCH_TAGS);
    }

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
