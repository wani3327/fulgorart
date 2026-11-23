//! Reading gallery-dl metadata (one file's "kwdict": a `--write-metadata`
//! sidecar, or the third element of a `-J` type-3 message).
//!
//! Deliberately loose: only the fields fulgorart uses are read, and all but
//! `category` and `id` are optional, so a gallery-dl update that adds, drops or
//! retypes a field doesn't break imports. Pixiv is mapped exactly; other sites
//! get a best-effort generic mapping.

use anyhow::{bail, Result};
use serde_json::{Map, Value};

use crate::import::{ImportAuthor, ImportPost};

/// What an image's metadata says about it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Entry {
    pub post: ImportPost,
    /// URL the file was downloaded from.
    pub source_url: Option<String>,
}

/// Keys kept in `post.raw_json` when present: what's useful later (tags,
/// ratings) without storing the whole, mostly redundant, kwdict.
const RAW_KEYS: &[&str] = &[
    "tags",
    "rating",
    "x_restrict",
    "sanity_level",
    "illust_ai_type",
    "illust_book_style",
    "type",
    "series",
    "tools",
    "page_count",
];

pub(super) fn parse(kwdict: &Value) -> Result<Entry> {
    let Some(kw) = kwdict.as_object() else {
        bail!("metadata is not a JSON object");
    };
    let Some(category) = string(kw, "category") else {
        bail!("metadata has no category");
    };
    let Some(id) = string(kw, "id").or_else(|| string(kw, "post_id")) else {
        bail!("{category} metadata has no post id");
    };

    let author = author(&category, kw);
    let source_post_url = post_url(&category, &id, kw);
    let raw: Map<String, Value> = RAW_KEYS
        .iter()
        .filter_map(|k| kw.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect();

    Ok(Entry {
        post: ImportPost {
            source_type: category,
            source_post_id: id,
            source_post_url,
            posted_at: string(kw, "date"),
            title: string(kw, "title"),
            caption: ["caption", "content", "description"]
                .iter()
                .find_map(|k| string(kw, k)),
            raw_json: (!raw.is_empty()).then(|| Value::Object(raw).to_string()),
            author,
        },
        source_url: string(kw, "url"),
    })
}

/// A string or number field as a non-empty string.
fn string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    match obj.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn post_url(category: &str, id: &str, kw: &Map<String, Value>) -> String {
    match category {
        "pixiv" => format!("https://www.pixiv.net/artworks/{id}"),
        "fanbox" => match string(kw, "creatorId") {
            Some(creator) => format!("https://www.fanbox.cc/@{creator}/posts/{id}"),
            None => format!("fanbox:{id}"),
        },
        // "url" is the file URL, not the post, so it isn't a fallback here
        _ => ["post_url", "permalink"]
            .iter()
            .find_map(|k| string(kw, k))
            .unwrap_or_else(|| format!("{category}:{id}")),
    }
}

fn author(category: &str, kw: &Map<String, Value>) -> Option<ImportAuthor> {
    // pixiv and most extractors: "user"; some use "author" or "creator"
    let obj = ["user", "author", "creator"]
        .iter()
        .find_map(|k| kw.get(*k).and_then(Value::as_object))?;
    let id = string(obj, "id").or_else(|| string(obj, "userId"))?;
    let name = ["name", "full_name", "nick"].iter().find_map(|k| string(obj, k));
    let (url, profile_url) = if category == "pixiv" {
        (
            Some(format!("https://www.pixiv.net/users/{id}")),
            obj.get("profile_image_urls")
                .and_then(Value::as_object)
                .and_then(|urls| string(urls, "medium")),
        )
    } else {
        (string(obj, "url"), None)
    };
    Some(ImportAuthor {
        source_author_id: id,
        name,
        url,
        profile_url,
    })
}

/// File name (without extension) gallery-dl gives a pixiv file by default:
/// metadata `filename` carries a hash (`123-<md5>_p0`) that the file on disk
/// (`123_p0.png`) doesn't. Used to match `-J` entries to files.
pub(super) fn disk_stem(kwdict: &Value) -> Option<String> {
    let filename = kwdict.get("filename")?.as_str()?;
    let has_hash = kwdict
        .get("hash")
        .and_then(Value::as_str)
        .is_some_and(|h| !h.is_empty());
    if has_hash {
        if let Some((left, page)) = filename.rsplit_once('_') {
            if let Some((name, _hash)) = left.rsplit_once('-') {
                return Some(format!("{name}_{page}"));
            }
        }
    }
    Some(filename.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pixiv() -> Value {
        // trimmed from a real gallery-dl 1.32.8 pixiv bookmark sidecar
        json!({
            "category": "pixiv", "subcategory": "favorite", "id": 138605452, "num": 0,
            "title": "猫", "caption": "猫猫", "date": "2025-12-14 12:11:03",
            "filename": "138605452-0ce329b0f12d7125d2ed6cbffbf60772_p0",
            "hash": "0ce329b0f12d7125d2ed6cbffbf60772", "extension": "png",
            "url": "https://i.pximg.net/img-original/img/2025/12/14/21/11/03/138605452_p0.png",
            "tags": ["猫", "オリジナル"], "x_restrict": 0, "rating": "General",
            "sanity_level": 2, "illust_ai_type": 1, "type": "illust",
            "total_view": 1234, "is_tag_full_lock": false,
            "user": {"id": 42, "name": "artist", "account": "acct",
                     "profile_image_urls": {"medium": "https://i.pximg.net/u.png"}}
        })
    }

    #[test]
    fn maps_pixiv_metadata() {
        let entry = parse(&pixiv()).unwrap();
        let post = &entry.post;
        assert_eq!(post.source_type, "pixiv");
        assert_eq!(post.source_post_id, "138605452");
        assert_eq!(post.source_post_url, "https://www.pixiv.net/artworks/138605452");
        assert_eq!(post.posted_at.as_deref(), Some("2025-12-14 12:11:03"));
        assert_eq!(post.caption.as_deref(), Some("猫猫"));
        let author = post.author.as_ref().unwrap();
        assert_eq!(author.source_author_id, "42");
        assert_eq!(author.url.as_deref(), Some("https://www.pixiv.net/users/42"));
        assert_eq!(author.profile_url.as_deref(), Some("https://i.pximg.net/u.png"));
        assert!(entry.source_url.unwrap().ends_with("138605452_p0.png"));
        let raw: Value = serde_json::from_str(post.raw_json.as_deref().unwrap()).unwrap();
        assert_eq!(raw["tags"][1], "オリジナル");
        assert!(raw.get("total_view").is_none(), "noisy fields aren't kept");
    }

    #[test]
    fn tolerates_missing_and_retyped_fields() {
        let mut kw = pixiv();
        let obj = kw.as_object_mut().unwrap();
        for k in ["title", "caption", "date", "user", "tags", "rating"] {
            obj.remove(k);
        }
        obj.insert("id".into(), json!("138605452")); // string instead of number
        obj.insert("brand_new_field".into(), json!({"x": 1}));
        let entry = parse(&kw).unwrap();
        assert_eq!(entry.post.source_post_id, "138605452");
        assert!(entry.post.author.is_none());
        assert!(entry.post.title.is_none());
    }

    #[test]
    fn requires_category_and_id() {
        assert!(parse(&json!({"id": 1})).is_err());
        assert!(parse(&json!({"category": "pixiv"})).is_err());
        assert!(parse(&json!([1, 2])).is_err());
    }

    #[test]
    fn generic_sites_get_best_effort_fields() {
        let entry = parse(&json!({
            "category": "fanbox", "id": "12296970", "creatorId": "shimo3620",
            "title": "post", "user": {"userId": "777", "name": "shimo"}
        }))
        .unwrap();
        assert_eq!(entry.post.source_post_url, "https://www.fanbox.cc/@shimo3620/posts/12296970");
        assert_eq!(entry.post.author.unwrap().source_author_id, "777");

        let other = parse(&json!({"category": "somesite", "id": 5})).unwrap();
        assert_eq!(other.post.source_post_url, "somesite:5");
    }

    /// Real-data check, run on demand:
    /// `FULGORART_GDL_DUMP=pixiv.json cargo test -p fulgorart-cli -- --ignored real_dump`
    /// Every type-3 entry of a real `gallery-dl -J` dump must parse; prints the
    /// disk stems (one per line) to compare against stored filenames.
    #[test]
    #[ignore]
    fn real_dump_parses() {
        let Ok(path) = std::env::var("FULGORART_GDL_DUMP") else { return };
        let dump: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut stems = Vec::new();
        for message in dump.iter().filter(|m| m[0] == 3) {
            let entry = parse(&message[2]).unwrap_or_else(|e| panic!("{e:#}: {}", message[2]));
            assert!(entry.post.author.is_some(), "pixiv entries have a user");
            stems.push(disk_stem(&message[2]).unwrap());
        }
        if let Ok(out) = std::env::var("FULGORART_GDL_STEMS_OUT") {
            std::fs::write(out, stems.join("\n")).unwrap();
        }
        println!("parsed {} entries", stems.len());
    }

    #[test]
    fn disk_stem_drops_pixiv_hash() {
        assert_eq!(disk_stem(&pixiv()).as_deref(), Some("138605452_p0"));
        assert_eq!(disk_stem(&json!({"filename": "a-b_p1", "hash": ""})).as_deref(), Some("a-b_p1"));
    }
}
