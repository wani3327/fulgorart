#![allow(unused)]
use serde::{Deserialize, Serialize};

use super::{ItemInterest, ItemInterested, PostInterest, PostInterested, UserInterested};

pub type PixivGalleryDlJson3 = Vec<(i64, String, PixivItem)>;
pub type PixivGalleryDlJson2 = Vec<(i64, PixivPost)>;

#[derive(Clone, Serialize, Deserialize)]
pub struct PixivItem {
    #[serde(flatten)]
    pixiv2: PixivPost,
    extension: String,
    filename: String,
    url: String,

    // No useful information -> dropped
    date_url: String,
    hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PixivPost {
    // common
    category: String,
    date: String,
    subcategory: String,

    // interested
    user: User,
    id: i64,
    caption: String,
    title: String,

    // information may help tagging -> compressed
    illust_ai_type: i64,
    rating: Rating,
    sanity_level: i64,
    tags: Vec<String>,
    x_restrict: i64,

    // information may read future -> compressed
    series: Option<Series>,
    tools: Vec<String>,
    illust_book_style: i64,
    #[serde(rename = "type")]
    pixiv_type: Type,

    // Some kind of information; but no need to store -> dropped
    total_bookmarks: i64,
    total_view: i64,
    request: Option<Request>,

    // No useful information -> dropped
    create_date: String, // duplicated
    is_bookmarked: bool,
    is_muted: bool,
    event_banners: Option<serde_json::Value>,
    num: i64,
    count: i64,
    page_count: i64,
    restrict: i64,
    restriction_attributes: Option<Vec<String>>,
    seasonal_effect_animation_urls: Option<serde_json::Value>,
    height: i64,
    suffix: String,
    width: i64,
    user_bookmark: UserBookmark,
    visible: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    Illust,
    Manga,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Rating {
    General,
    #[serde(rename = "R-18")]
    R18,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    request_info: RequestInfo,
    request_users: Vec<Option<serde_json::Value>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RequestInfo {
    collaborate_status: CollaborateStatus,
    fan_user_id: Option<serde_json::Value>,
    role: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CollaborateStatus {
    collaborate_anonymous_flag: bool,
    collaborate_user_samples: Vec<Option<serde_json::Value>>,
    collaborating: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Series {
    id: i64,
    title: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    id: i64,
    name: String,
    profile_image_urls: ProfileImageUrls,

    // Some kind of information; but no need to store -> dropped
    account: String,
    is_accept_request: bool,
    is_followed: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProfileImageUrls {
    medium: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UserBookmark {
    account: String,
    comment: String,
    id: i64,
    is_access_blocking_user: bool,
    is_followed: bool,
    name: String,
    profile_image_urls: ProfileImageUrls,
}

#[derive(Serialize)]
struct Pixiv3Compressed {
    illust_ai_type: i64,
    rating: Rating,
    sanity_level: i64,
    tags: Vec<String>,
    x_restrict: i64,
    series: Option<Series>,
    tools: Vec<String>,
    illust_book_style: i64,
    #[serde(rename = "type")]
    pixiv_type: Type,
}

impl PostInterest for (i64, String, PixivItem) {
    fn post(self) -> PostInterested {
        let user = UserInterested {
            id: self.2.pixiv2.user.id,
            name: self.2.pixiv2.user.name,
            url: format!("https://www.pixiv.net/users/{}", self.2.pixiv2.user.id),
            profile_url: self.2.pixiv2.user.profile_image_urls.medium,
        };

        let compressed = Pixiv3Compressed {
            illust_ai_type: self.2.pixiv2.illust_ai_type,
            rating: self.2.pixiv2.rating,
            sanity_level: self.2.pixiv2.sanity_level,
            tags: self.2.pixiv2.tags,
            x_restrict: self.2.pixiv2.x_restrict,
            series: self.2.pixiv2.series,
            tools: self.2.pixiv2.tools,
            illust_book_style: self.2.pixiv2.illust_book_style,
            pixiv_type: self.2.pixiv2.pixiv_type,
        };

        PostInterested {
            category: self.2.pixiv2.category,
            date: self.2.pixiv2.date,
            user,
            id: self.2.pixiv2.id,
            caption: self.2.pixiv2.caption,
            title: self.2.pixiv2.title,
            compressed: serde_json::to_vec(&compressed)
                .expect("failed to serialize compressed pixiv fields"),
        }
    }
}

impl ItemInterest for (i64, String, PixivItem) {
    fn item(self) -> ItemInterested {
        ItemInterested {
            extension: self.2.extension,
            filename: self.2.filename,
            url: self.1,
        }
    }
}
