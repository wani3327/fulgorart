use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct DbConfig {
    pub path: PathBuf,
}

impl DbConfig {
    pub fn from_env() -> Self {
        let path = if let Ok(s) = std::env::var("FULGORART_DB_PATH") {
            PathBuf::from(s)
        } else if let Ok(s) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(s).join("fulgorart.db")
        } else if let Ok(s) = std::env::var("HOME") {
            PathBuf::from(s).join(".local/share/fulgorart.db")
        } else {
            PathBuf::from("./fulgorart.db")
        };

        Self { path }
    }
}

use sqlx::SqlitePool;
use tracing::instrument;

#[derive(Debug, Clone)]
pub struct Db {
    pub pool: SqlitePool,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct AuthorRow {
    pub id: i64,
    pub source_type: String,
    pub source_author_id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub profile_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct PostRow {
    pub id: i64,
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub author_id: Option<i64>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub uploaded_at: Option<String>,
    pub raw_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct ImageAssetRow {
    pub id: i64,
    pub post_id: Option<i64>,
    pub sha256: String,
    pub s3_key_base: String,
    pub filename: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub file_size: Option<i64>,
    pub content_type: String,
    pub source_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct TagRow {
    pub id: i64,
    pub name: String,
    pub category: Option<String>,
    pub created_at: String,
}

/// One gallery entry when folding by post: the cover image plus group size.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ImageGroupRow {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub asset: ImageAssetRow,
    /// Images of this post that match the filter (1 = nothing to fold).
    pub group_size: i64,
}

/// `SELECT ia.* FROM image_asset ia ...` restricted to images having every
/// `include` tag and none of the `exclude` tags. Bind with `bind_tag_filter`.
fn tag_filter_sql(include: &[String], exclude: &[String]) -> String {
    let mut sql = String::from("SELECT DISTINCT ia.* FROM image_asset ia");
    for i in 0..include.len() {
        sql.push_str(&format!(
            " JOIN image_tag it{i} ON ia.id = it{i}.image_id
              JOIN tag t{i} ON it{i}.tag_id = t{i}.id AND t{i}.name = ?"
        ));
    }
    if !exclude.is_empty() {
        let placeholders = vec!["?"; exclude.len()].join(", ");
        sql.push_str(&format!(
            " WHERE ia.id NOT IN (
                SELECT image_id FROM image_tag it_ex
                JOIN tag t_ex ON it_ex.tag_id = t_ex.id
                WHERE t_ex.name IN ({placeholders}))"
        ));
    }
    sql
}

/// Bind the parameters of `tag_filter_sql`, in its order.
fn bind_tag_filter<'q, O>(
    mut query: sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>>,
    include: &'q [String],
    exclude: &'q [String],
) -> sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>> {
    for tag in include.iter().chain(exclude) {
        query = query.bind(tag);
    }
    query
}

/// A tag suggestion for search autocomplete.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, serde::Serialize)]
pub struct TagSuggestion {
    pub name: String,
    pub category: Option<String>,
    pub image_count: i64,
}

/// Escape `%`, `_` and `\` so user text matches literally in `LIKE ... ESCAPE '\'`
/// (tag names are full of underscores).
fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct ImageTagRow {
    pub id: i64,
    pub image_id: i64,
    pub tag_id: i64,
    pub source: String,
    pub score: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct TagJobRow {
    pub id: i64,
    pub image_id: i64,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct SourceAccountRow {
    pub id: i64,
    pub source_type: String,
    pub account_id: String,
    pub display_name: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagJobWithKey {
    pub job_id: i64,
    pub image_id: i64,
    pub s3_key_base: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageAssetWithTags {
    #[serde(flatten)]
    pub asset: ImageAssetRow,
    pub tags: Vec<TagRow>,
}

#[derive(Debug, Clone)]
pub struct ClaimedImageUpload {
    pub asset: ImageAssetRow,
    pub job: TagJobRow,
}

#[derive(Debug, Clone)]
pub struct GalleryImportPostArgs<'a> {
    pub source_type: &'a str,
    pub source_post_id: &'a str,
    pub source_post_url: &'a str,
    pub uploaded_at: Option<&'a str>,
    pub title: Option<&'a str>,
    pub caption: Option<&'a str>,
    pub raw_json: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct GalleryImportAuthorArgs<'a> {
    pub source_author_id: &'a str,
    pub name: Option<&'a str>,
    pub url: Option<&'a str>,
    pub profile_url: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct GalleryImportImageArgs<'a> {
    pub sha256: &'a str,
    pub s3_key_base: &'a str,
    pub filename: Option<&'a str>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub file_size: Option<i64>,
    pub content_type: &'a str,
    pub source_url: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct GalleryImportClaimResult {
    pub post: PostRow,
    pub inserted_post: bool,
    pub claimed_upload: Option<ClaimedImageUpload>,
}

impl Db {
    #[instrument(skip(path))]
    pub async fn connect(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", path.display())).await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Db { pool })
    }

    // ---- Post / Author ----

    pub async fn upsert_author(
        &self,
        source_type: &str,
        source_author_id: &str,
        name: Option<&str>,
        url: Option<&str>,
    ) -> Result<AuthorRow> {
        sqlx::query(
            "INSERT INTO author (source_type, source_author_id, name, url)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(source_type, source_author_id) DO UPDATE SET
               name = COALESCE(excluded.name, author.name),
               url = COALESCE(excluded.url, author.url),
               updated_at = datetime('now')",
        )
        .bind(source_type)
        .bind(source_author_id)
        .bind(name)
        .bind(url)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, AuthorRow>(
            "SELECT * FROM author WHERE source_type = ? AND source_author_id = ?",
        )
        .bind(source_type)
        .bind(source_author_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn upsert_author_with_profile(
        &self,
        source_type: &str,
        source_author_id: &str,
        name: Option<&str>,
        url: Option<&str>,
        profile_url: Option<&str>,
    ) -> Result<AuthorRow> {
        sqlx::query(
            "INSERT INTO author (source_type, source_author_id, name, url, profile_url)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(source_type, source_author_id) DO UPDATE SET
               name = COALESCE(excluded.name, author.name),
               url = COALESCE(excluded.url, author.url),
               profile_url = COALESCE(excluded.profile_url, author.profile_url),
               updated_at = datetime('now')",
        )
        .bind(source_type)
        .bind(source_author_id)
        .bind(name)
        .bind(url)
        .bind(profile_url)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, AuthorRow>(
            "SELECT * FROM author WHERE source_type = ? AND source_author_id = ?",
        )
        .bind(source_type)
        .bind(source_author_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn insert_post(
        &self,
        post: &GalleryImportPostArgs<'_>,
        author: Option<&GalleryImportAuthorArgs<'_>>,
        // source_type: &str,
        // source_post_id: &str,
        // source_post_url: &str,
        // uploaded_at: Option<&str>,
        // author_source_id: Option<&str>,
        // author_name: Option<&str>,
        // author_url: Option<&str>,
        // author_profile_url: Option<&str>,
        // title: Option<&str>,
        // caption: Option<&str>,
        // raw_json: Option<&str>,
    ) -> Result<PostRow> {
        let author_id = match author {
            Some(author) => Some(
                self.upsert_author_with_profile(
                    post.source_type,
                    author.source_author_id,
                    author.name,
                    author.url,
                    author.profile_url,
                )
                .await?
                .id,
            ),
            None => None,
        };

        sqlx::query(
            "INSERT INTO post (source_type, source_post_id, source_post_url, author_id, title, caption, uploaded_at, raw_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(source_type, source_post_id) DO UPDATE SET
               source_post_url = excluded.source_post_url,
               author_id = excluded.author_id,
               title = excluded.title,
               caption = excluded.caption,
               uploaded_at = excluded.uploaded_at,
               raw_json = excluded.raw_json"
        )
        .bind(post.source_type)
        .bind(post.source_post_id)
        .bind(post.source_post_url)
        .bind(author_id)
        .bind(post.title)
        .bind(post.caption)
        .bind(post.uploaded_at)
        .bind(post.raw_json)
        .execute(&self.pool)
        .await?;

        self.get_post_by_source(post.source_type, post.source_post_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Post not found after insert"))
    }

    pub async fn get_post_by_id(&self, id: i64) -> Result<Option<PostRow>> {
        let row = sqlx::query_as::<_, PostRow>("SELECT * FROM post WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn get_post_by_source(
        &self,
        source_type: &str,
        source_post_id: &str,
    ) -> Result<Option<PostRow>> {
        let row = sqlx::query_as::<_, PostRow>(
            "SELECT * FROM post WHERE source_type = ? AND source_post_id = ?",
        )
        .bind(source_type)
        .bind(source_post_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn claim_gallery_image_upload(
        &self,
        post: &GalleryImportPostArgs<'_>,
        author: Option<&GalleryImportAuthorArgs<'_>>,
        image: &GalleryImportImageArgs<'_>,
    ) -> Result<GalleryImportClaimResult> {
        let (post_row, inserted_post) = match self
            .get_post_by_source(post.source_type, post.source_post_id)
            .await?
        {
            Some(existing) => (existing, false),
            None => {
                let inserted = self.insert_post(post, author).await?;
                (inserted, true)
            }
        };

        let claimed_upload = self
            .claim_image_asset_upload_with_filename(
                Some(post_row.id),
                image.sha256,
                image.s3_key_base,
                image.filename,
                image.width,
                image.height,
                image.file_size,
                image.content_type,
                image.source_url,
            )
            .await?;

        Ok(GalleryImportClaimResult {
            post: post_row,
            inserted_post,
            claimed_upload,
        })
    }

    // ---- ImageAsset ----

    pub async fn insert_image_asset(
        &self,
        post_id: Option<i64>,
        sha256: &str,
        s3_key_base: &str,
        width: Option<i64>,
        height: Option<i64>,
        file_size: Option<i64>,
        content_type: &str,
        source_url: Option<&str>,
    ) -> Result<ImageAssetRow> {
        self.insert_image_asset_with_filename(
            post_id,
            sha256,
            s3_key_base,
            None,
            width,
            height,
            file_size,
            content_type,
            source_url,
        )
        .await
    }

    pub async fn insert_image_asset_with_filename(
        &self,
        post_id: Option<i64>,
        sha256: &str,
        s3_key_base: &str,
        filename: Option<&str>,
        width: Option<i64>,
        height: Option<i64>,
        file_size: Option<i64>,
        content_type: &str,
        source_url: Option<&str>,
    ) -> Result<ImageAssetRow> {
        sqlx::query(
            "INSERT INTO image_asset (post_id, sha256, s3_key_base, filename, width, height, file_size, content_type, source_url)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(sha256) DO UPDATE SET
                s3_key_base = excluded.s3_key_base,
                post_id = COALESCE(image_asset.post_id, excluded.post_id),
                filename = COALESCE(excluded.filename, image_asset.filename),
                source_url = COALESCE(excluded.source_url, image_asset.source_url),
                updated_at = datetime('now')"
        )
        .bind(post_id)
        .bind(sha256)
        .bind(s3_key_base)
        .bind(filename)
        .bind(width)
        .bind(height)
        .bind(file_size)
        .bind(content_type)
        .bind(source_url)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, ImageAssetRow>("SELECT * FROM image_asset WHERE sha256 = ?")
            .bind(sha256)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn get_image_asset_by_id(&self, id: i64) -> Result<Option<ImageAssetRow>> {
        let row = sqlx::query_as::<_, ImageAssetRow>("SELECT * FROM image_asset WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn get_image_asset_by_sha256(&self, sha256: &str) -> Result<Option<ImageAssetRow>> {
        let row = sqlx::query_as::<_, ImageAssetRow>("SELECT * FROM image_asset WHERE sha256 = ?")
            .bind(sha256)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn claim_image_asset_upload_with_filename(
        &self,
        post_id: Option<i64>,
        sha256: &str,
        s3_key_base: &str,
        filename: Option<&str>,
        width: Option<i64>,
        height: Option<i64>,
        file_size: Option<i64>,
        content_type: &str,
        source_url: Option<&str>,
    ) -> Result<Option<ClaimedImageUpload>> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT OR IGNORE INTO image_asset (post_id, sha256, s3_key_base, filename, width, height, file_size, content_type, source_url)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(post_id)
        .bind(sha256)
        .bind(s3_key_base)
        .bind(filename)
        .bind(width)
        .bind(height)
        .bind(file_size)
        .bind(content_type)
        .bind(source_url)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(None);
        }

        let asset =
            sqlx::query_as::<_, ImageAssetRow>("SELECT * FROM image_asset WHERE sha256 = ?")
                .bind(sha256)
                .fetch_one(&mut *tx)
                .await?;
        let job_result =
            sqlx::query("INSERT INTO tag_job (image_id, status) VALUES (?, 'uploading')")
                .bind(asset.id)
                .execute(&mut *tx)
                .await?;
        let job = sqlx::query_as::<_, TagJobRow>("SELECT * FROM tag_job WHERE id = ?")
            .bind(job_result.last_insert_rowid())
            .fetch_one(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(Some(ClaimedImageUpload { asset, job }))
    }

    pub async fn list_image_assets(&self, page: i64, per_page: i64) -> Result<Vec<ImageAssetRow>> {
        let offset = (page - 1) * per_page;
        let rows = sqlx::query_as::<_, ImageAssetRow>(
            "SELECT * FROM image_asset ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(per_page)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_image_assets_by_tags(
        &self,
        include_tags: &[String],
        exclude_tags: &[String],
        page: i64,
        per_page: i64,
    ) -> Result<Vec<ImageAssetRow>> {
        let offset = (page - 1) * per_page;

        if include_tags.is_empty() && exclude_tags.is_empty() {
            return self.list_image_assets(page, per_page).await;
        }

        let sql = format!(
            "{} ORDER BY ia.created_at DESC LIMIT ? OFFSET ?",
            tag_filter_sql(include_tags, exclude_tags)
        );
        let q = bind_tag_filter(sqlx::query_as::<_, ImageAssetRow>(&sql), include_tags, exclude_tags);
        Ok(q.bind(per_page).bind(offset).fetch_all(&self.pool).await?)
    }

    /// Gallery page folded by post: one row per post (images without a post
    /// are their own group), newest group first. Each row is the group's
    /// cover, the image with the most explicit rating (ties: first image of
    /// the post), plus how many of the post's images match the filter.
    /// Takes `limit`/`offset` rather than a page so callers can fetch one extra
    /// row to learn whether there's a next page without shifting the offset.
    pub async fn list_image_groups(
        &self,
        include_tags: &[String],
        exclude_tags: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ImageGroupRow>> {
        let sql = format!(
            "WITH f AS ({filter}),
             r AS (
               SELECT f.*, COALESCE(f.post_id, -f.id) AS grp,
                 COALESCE((
                   SELECT MAX(CASE t.name WHEN 'explicit' THEN 3 WHEN 'questionable' THEN 2
                                          WHEN 'sensitive' THEN 1 WHEN 'general' THEN 0 END)
                   FROM image_tag it JOIN tag t ON t.id = it.tag_id
                   WHERE it.image_id = f.id AND t.category = 'rating'
                 ), -1) AS rating_rank
               FROM f),
             w AS (
               SELECT r.*,
                 ROW_NUMBER() OVER (PARTITION BY grp ORDER BY rating_rank DESC, id ASC) AS rn,
                 COUNT(*) OVER (PARTITION BY grp) AS group_size,
                 MAX(created_at) OVER (PARTITION BY grp) AS group_created,
                 MAX(id) OVER (PARTITION BY grp) AS group_last_id
               FROM r)
             SELECT id, post_id, sha256, s3_key_base, filename, width, height, file_size,
                    content_type, source_url, created_at, updated_at, group_size
             FROM w WHERE rn = 1
             ORDER BY group_created DESC, group_last_id DESC
             LIMIT ? OFFSET ?",
            filter = tag_filter_sql(include_tags, exclude_tags)
        );
        let q = bind_tag_filter(sqlx::query_as::<_, ImageGroupRow>(&sql), include_tags, exclude_tags);
        Ok(q.bind(limit).bind(offset).fetch_all(&self.pool).await?)
    }

    /// Images of one post that match the filter, in post order.
    pub async fn list_post_images(
        &self,
        post_id: i64,
        include_tags: &[String],
        exclude_tags: &[String],
    ) -> Result<Vec<ImageAssetRow>> {
        let sql = format!(
            "SELECT * FROM ({}) f WHERE f.post_id = ? ORDER BY f.id",
            tag_filter_sql(include_tags, exclude_tags)
        );
        let q = bind_tag_filter(sqlx::query_as::<_, ImageAssetRow>(&sql), include_tags, exclude_tags);
        Ok(q.bind(post_id).fetch_all(&self.pool).await?)
    }

    // ---- Tag ----

    pub async fn insert_tag(&self, name: &str, category: Option<&str>) -> Result<TagRow> {
        let result = sqlx::query(
            "INSERT INTO tag (name, category) VALUES (?, ?)
             ON CONFLICT(name) DO UPDATE SET category = COALESCE(excluded.category, tag.category)",
        )
        .bind(name)
        .bind(category)
        .execute(&self.pool)
        .await?;

        let id = result.last_insert_rowid();
        if id == 0 {
            self.get_tag_by_name(name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Tag not found after insert"))
        } else {
            sqlx::query_as::<_, TagRow>("SELECT * FROM tag WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(Into::into)
        }
    }

    pub async fn get_or_create_tag(&self, name: &str, category: Option<&str>) -> Result<TagRow> {
        if let Some(tag) = self.get_tag_by_name(name).await? {
            return Ok(tag);
        }
        self.insert_tag(name, category).await
    }

    pub async fn get_tag_by_name(&self, name: &str) -> Result<Option<TagRow>> {
        let row = sqlx::query_as::<_, TagRow>("SELECT * FROM tag WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    /// Tags matching `query` for search autocomplete: names starting with it
    /// first, then names containing it, each by how many images use the tag.
    /// Tags no image uses are left out.
    pub async fn suggest_tags(&self, query: &str, limit: i64) -> Result<Vec<TagSuggestion>> {
        let escaped = escape_like(query);
        let rows = sqlx::query_as::<_, TagSuggestion>(
            "SELECT t.name, t.category, COUNT(it.image_id) AS image_count
             FROM tag t JOIN image_tag it ON it.tag_id = t.id
             WHERE t.name LIKE ?1 ESCAPE '\\'
             GROUP BY t.id
             ORDER BY (t.name LIKE ?2 ESCAPE '\\') DESC, image_count DESC, t.name
             LIMIT ?3",
        )
        .bind(format!("%{escaped}%"))
        .bind(format!("{escaped}%"))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn search_tags(&self, query: &str) -> Result<Vec<TagRow>> {
        let pattern = format!("%{}%", query);
        let rows = sqlx::query_as::<_, TagRow>(
            "SELECT * FROM tag WHERE name LIKE ? ORDER BY name LIMIT 50",
        )
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_all_tags(&self) -> Result<Vec<TagRow>> {
        let rows = sqlx::query_as::<_, TagRow>("SELECT * FROM tag ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    // ---- ImageTag ----

    pub async fn insert_image_tag(
        &self,
        image_id: i64,
        tag_id: i64,
        source: &str,
        score: Option<f64>,
    ) -> Result<ImageTagRow> {
        let result = sqlx::query(
            "INSERT INTO image_tag (image_id, tag_id, source, score) VALUES (?, ?, ?, ?)
             ON CONFLICT(image_id, tag_id) DO UPDATE SET source = excluded.source, score = excluded.score"
        )
        .bind(image_id)
        .bind(tag_id)
        .bind(source)
        .bind(score)
        .execute(&self.pool)
        .await?;

        let id = result.last_insert_rowid();
        let row = if id == 0 {
            sqlx::query_as::<_, ImageTagRow>(
                "SELECT * FROM image_tag WHERE image_id = ? AND tag_id = ?",
            )
            .bind(image_id)
            .bind(tag_id)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, ImageTagRow>("SELECT * FROM image_tag WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?
        };
        Ok(row)
    }

    pub async fn delete_image_tag(&self, image_id: i64, tag_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM image_tag WHERE image_id = ? AND tag_id = ?")
            .bind(image_id)
            .bind(tag_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_image_tags(&self, image_id: i64) -> Result<Vec<TagRow>> {
        let rows = sqlx::query_as::<_, TagRow>(
            "SELECT t.* FROM tag t
             JOIN image_tag it ON t.id = it.tag_id
             WHERE it.image_id = ?
             ORDER BY t.name",
        )
        .bind(image_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ---- TagJob ----

    pub async fn insert_tag_job(&self, image_id: i64) -> Result<TagJobRow> {
        let result = sqlx::query("INSERT INTO tag_job (image_id, status) VALUES (?, 'uploaded')")
            .bind(image_id)
            .execute(&self.pool)
            .await?;

        let id = result.last_insert_rowid();
        sqlx::query_as::<_, TagJobRow>("SELECT * FROM tag_job WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn get_pending_tag_jobs(&self, limit: i64) -> Result<Vec<TagJobRow>> {
        let rows = sqlx::query_as::<_, TagJobRow>(
            "SELECT * FROM tag_job WHERE status = 'uploaded' ORDER BY created_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn ensure_tag_job(&self, image_id: i64) -> Result<TagJobRow> {
        if let Some(existing) = sqlx::query_as::<_, TagJobRow>(
            "SELECT * FROM tag_job WHERE image_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(image_id)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(existing);
        }

        self.insert_tag_job(image_id).await
    }

    pub async fn get_pending_tag_jobs_with_keys(&self, limit: i64) -> Result<Vec<TagJobWithKey>> {
        let rows = sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT tj.id, tj.image_id, ia.s3_key_base
             FROM tag_job tj
             JOIN image_asset ia ON tj.image_id = ia.id
             WHERE tj.status = 'uploaded'
             ORDER BY tj.created_at ASC
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(job_id, image_id, s3_key_base)| TagJobWithKey {
                job_id,
                image_id,
                s3_key_base,
            })
            .collect())
    }

    pub async fn update_tag_job_status(
        &self,
        id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE tag_job SET status = ?, error = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(status)
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_db(name: &str) -> Db {
        let path = std::env::temp_dir().join(format!(
            "fulgorart-db-test-{name}-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        Db::connect(&path).await.unwrap()
    }

    /// Images 1..=n, each with the given tags.
    async fn seed(db: &Db, images: &[&[&str]]) -> Vec<i64> {
        let mut ids = Vec::new();
        for (i, tags) in images.iter().enumerate() {
            // the path importers use (insert_image_asset's SQL is currently broken)
            let asset = db
                .claim_image_asset_upload_with_filename(
                    None, &format!("sha{i}"), &format!("k/{i}"), None, None, None, None, "image/webp", None,
                )
                .await
                .unwrap()
                .expect("new sha256 is claimed")
                .asset;
            for tag in *tags {
                let tag = db.get_or_create_tag(tag, Some("general")).await.unwrap();
                db.insert_image_tag(asset.id, tag.id, "wd14", Some(0.9)).await.unwrap();
            }
            ids.push(asset.id);
        }
        ids
    }

    async fn post(db: &Db, id: &str) -> i64 {
        db.insert_post(
            &GalleryImportPostArgs {
                source_type: "pixiv",
                source_post_id: id,
                source_post_url: "https://example.invalid",
                uploaded_at: None,
                title: None,
                caption: None,
                raw_json: None,
            },
            None,
        )
        .await
        .unwrap()
        .id
    }

    /// Image in `post_id` with `tags`; returns its id.
    async fn image(db: &Db, post_id: Option<i64>, key: &str, tags: &[&str]) -> i64 {
        let asset = db
            .claim_image_asset_upload_with_filename(post_id, key, key, None, None, None, None, "image/webp", None)
            .await
            .unwrap()
            .expect("new sha256 is claimed")
            .asset;
        for tag in tags {
            let category = if ["general", "sensitive", "questionable", "explicit"].contains(tag) { "rating" } else { "general" };
            let tag = db.get_or_create_tag(tag, Some(category)).await.unwrap();
            db.insert_image_tag(asset.id, tag.id, "wd14", Some(0.9)).await.unwrap();
        }
        asset.id
    }

    #[tokio::test]
    async fn groups_fold_posts_with_most_explicit_cover() {
        let db = temp_db("groups").await;
        let a = post(&db, "a").await;
        let a0 = image(&db, Some(a), "a0", &["sensitive", "1girl"]).await;
        let a1 = image(&db, Some(a), "a1", &["explicit", "1girl"]).await;
        let _a2 = image(&db, Some(a), "a2", &["questionable"]).await;
        let lone = image(&db, None, "lone", &["general", "1girl"]).await;
        let b = post(&db, "b").await;
        let b0 = image(&db, Some(b), "b0", &["general", "1girl"]).await;
        let _b1 = image(&db, Some(b), "b1", &["general"]).await;

        let groups = db.list_image_groups(&[], &[], 50, 0).await.unwrap();
        let summary: Vec<(i64, i64)> = groups.iter().map(|g| (g.asset.id, g.group_size)).collect();
        // newest group first; cover = most explicit, ties -> first image of the post
        assert_eq!(summary, [(b0, 2), (lone, 1), (a1, 3)]);

        // with a filter, groups count only matching images and the cover is among them
        let with_1girl = db.list_image_groups(&["1girl".into()], &[], 50, 0).await.unwrap();
        let summary: Vec<(i64, i64)> = with_1girl.iter().map(|g| (g.asset.id, g.group_size)).collect();
        assert_eq!(summary, [(b0, 1), (lone, 1), (a1, 2)]);
        let not_explicit = db.list_image_groups(&[], &["explicit".into()], 50, 0).await.unwrap();
        assert_eq!(not_explicit.last().map(|g| (g.asset.id, g.group_size)), Some((_a2, 2)));

        // paging is by group; consecutive pages neither skip nor repeat
        let first = db.list_image_groups(&[], &[], 2, 0).await.unwrap();
        let rest = db.list_image_groups(&[], &[], 2, 2).await.unwrap();
        let paged: Vec<i64> = first.iter().chain(&rest).map(|g| g.asset.id).collect();
        assert_eq!(paged, [b0, lone, a1]);

        // post images in post order, filtered
        let ids = |rows: Vec<ImageAssetRow>| rows.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(db.list_post_images(a, &[], &[]).await.unwrap()), [a0, a1, _a2]);
        assert_eq!(ids(db.list_post_images(a, &["1girl".into()], &[]).await.unwrap()), [a0, a1]);
    }

    #[tokio::test]
    async fn suggest_tags_prefix_first_then_by_use() {
        let db = temp_db("suggest").await;
        seed(
            &db,
            &[
                &["pink_hair", "long_hair"],
                &["pink_hair", "hair_ornament"],
                &["long_hair"],
                &["long_hair"],
                &["pinkxhair"],
            ],
        )
        .await;

        let names = |s: Vec<TagSuggestion>| s.into_iter().map(|t| (t.name, t.image_count)).collect::<Vec<_>>();
        // prefix match (hair_ornament) before more popular substring matches
        assert_eq!(
            names(db.suggest_tags("hair", 10).await.unwrap()),
            [
                ("hair_ornament".into(), 1),
                ("long_hair".into(), 3),
                ("pink_hair".into(), 2),
                ("pinkxhair".into(), 1)
            ]
        );
        // '_' is literal, so "pink_" doesn't match "pinkxhair"
        assert_eq!(names(db.suggest_tags("pink_", 10).await.unwrap()), [("pink_hair".into(), 2)]);
        assert_eq!(db.suggest_tags("long", 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tag_search_includes_all_and_excludes_any() {
        let db = temp_db("search").await;
        let ids = seed(
            &db,
            &[&["1girl", "pink_hair"], &["1girl", "pink_hair", "explicit"], &["1girl"], &["pink_hair"]],
        )
        .await;
        let found = |rows: Vec<ImageAssetRow>| {
            let mut v: Vec<i64> = rows.into_iter().map(|r| r.id).collect();
            v.sort();
            v
        };
        let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();

        let both = db.list_image_assets_by_tags(&s(&["1girl", "pink_hair"]), &[], 1, 50).await.unwrap();
        assert_eq!(found(both), [ids[0], ids[1]]);
        let not_explicit = db
            .list_image_assets_by_tags(&s(&["1girl", "pink_hair"]), &s(&["explicit"]), 1, 50)
            .await
            .unwrap();
        assert_eq!(found(not_explicit), [ids[0]]);
        let only_exclude = db.list_image_assets_by_tags(&[], &s(&["pink_hair"]), 1, 50).await.unwrap();
        assert_eq!(found(only_exclude), [ids[2]]);
    }
}
