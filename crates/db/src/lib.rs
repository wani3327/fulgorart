use anyhow::Result;
use sqlx::SqlitePool;
use tracing::instrument;

#[derive(Debug, Clone)]
pub struct Db {
    pub pool: SqlitePool,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct PostRow {
    pub id: i64,
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub liked_at: Option<String>,
    pub author_name: Option<String>,
    pub author_id: Option<String>,
    pub raw_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct ImageAssetRow {
    pub id: i64,
    pub post_id: Option<i64>,
    pub group_id: Option<i64>,
    pub sha256: String,
    pub r2_key: String,
    pub r2_url: String,
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

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct ImageGroupRow {
    pub id: i64,
    pub post_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagJobWithUrl {
    pub job_id: i64,
    pub image_id: i64,
    pub r2_url: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageAssetWithTags {
    #[serde(flatten)]
    pub asset: ImageAssetRow,
    pub tags: Vec<TagRow>,
}

impl Db {
    #[instrument(skip(path))]
    pub async fn connect(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", path)).await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Db { pool })
    }

    // ---- Post ----

    pub async fn insert_post(
        &self,
        source_type: &str,
        source_post_id: &str,
        source_post_url: &str,
        liked_at: Option<&str>,
        author_name: Option<&str>,
        author_id: Option<&str>,
        raw_json: Option<&str>,
    ) -> Result<PostRow> {
        let result = sqlx::query(
            "INSERT INTO post (source_type, source_post_id, source_post_url, liked_at, author_name, author_id, raw_json)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(source_type, source_post_id) DO UPDATE SET
               source_post_url = excluded.source_post_url,
               liked_at = excluded.liked_at,
               author_name = excluded.author_name,
               author_id = excluded.author_id,
               raw_json = excluded.raw_json,
               updated_at = datetime('now')"
        )
        .bind(source_type)
        .bind(source_post_id)
        .bind(source_post_url)
        .bind(liked_at)
        .bind(author_name)
        .bind(author_id)
        .bind(raw_json)
        .execute(&self.pool)
        .await?;

        let id = result.last_insert_rowid();
        // If upsert updated an existing row, last_insert_rowid returns 0 or the existing row id
        // So we need to fetch by source
        if id == 0 {
            self.get_post_by_source(source_type, source_post_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Post not found after insert"))
        } else {
            self.get_post_by_id(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Post not found after insert"))
        }
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

    // ---- ImageAsset ----

    pub async fn insert_image_group(&self, post_id: Option<i64>) -> Result<ImageGroupRow> {
        match post_id {
            Some(post_id) => {
                sqlx::query(
                    "INSERT INTO image_group (post_id) VALUES (?)
                     ON CONFLICT(post_id) DO UPDATE SET
                       updated_at = datetime('now')",
                )
                .bind(post_id)
                .execute(&self.pool)
                .await?;

                sqlx::query_as::<_, ImageGroupRow>("SELECT * FROM image_group WHERE post_id = ?")
                    .bind(post_id)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(Into::into)
            }
            None => {
                let result = sqlx::query("INSERT INTO image_group (post_id) VALUES (NULL)")
                    .execute(&self.pool)
                    .await?;

                sqlx::query_as::<_, ImageGroupRow>("SELECT * FROM image_group WHERE id = ?")
                    .bind(result.last_insert_rowid())
                    .fetch_one(&self.pool)
                    .await
                    .map_err(Into::into)
            }
        }
    }

    pub async fn insert_image_asset(
        &self,
        post_id: Option<i64>,
        group_id: Option<i64>,
        sha256: &str,
        r2_key: &str,
        r2_url: &str,
        width: Option<i64>,
        height: Option<i64>,
        file_size: Option<i64>,
        content_type: &str,
        source_url: Option<&str>,
    ) -> Result<ImageAssetRow> {
        sqlx::query(
            "INSERT INTO image_asset (post_id, group_id, sha256, r2_key, r2_url, width, height, file_size, content_type, source_url)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(sha256) DO UPDATE SET
                r2_url = excluded.r2_url,
                group_id = COALESCE(image_asset.group_id, excluded.group_id),
                updated_at = datetime('now')"
        )
        .bind(post_id)
        .bind(group_id)
        .bind(sha256)
        .bind(r2_key)
        .bind(r2_url)
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

        // Build dynamic query
        let mut sql = String::from("SELECT DISTINCT ia.* FROM image_asset ia");

        for (i, _) in include_tags.iter().enumerate() {
            sql.push_str(&format!(
                " JOIN image_tag it{i} ON ia.id = it{i}.image_id
                  JOIN tag t{i} ON it{i}.tag_id = t{i}.id AND t{i}.name = ?"
            ));
        }

        if !exclude_tags.is_empty() {
            sql.push_str(
                " WHERE ia.id NOT IN (
                    SELECT image_id FROM image_tag it_ex
                    JOIN tag t_ex ON it_ex.tag_id = t_ex.id
                    WHERE t_ex.name IN (",
            );
            let placeholders = exclude_tags
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&placeholders);
            sql.push_str("))");
        }

        sql.push_str(" ORDER BY ia.created_at DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query_as::<_, ImageAssetRow>(&sql);
        for tag in include_tags {
            q = q.bind(tag);
        }
        for tag in exclude_tags {
            q = q.bind(tag);
        }
        q = q.bind(per_page).bind(offset);

        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows)
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
        let result = sqlx::query("INSERT INTO tag_job (image_id, status) VALUES (?, 'pending')")
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
            "SELECT * FROM tag_job WHERE status = 'pending' ORDER BY created_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_pending_tag_jobs_with_urls(
        &self,
        limit: i64,
    ) -> Result<Vec<TagJobWithUrl>> {
        let rows = sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT tj.id, tj.image_id, ia.r2_url
             FROM tag_job tj
             JOIN image_asset ia ON tj.image_id = ia.id
             WHERE tj.status = 'pending'
             ORDER BY tj.created_at ASC
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(job_id, image_id, r2_url)| TagJobWithUrl {
                job_id,
                image_id,
                r2_url,
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
