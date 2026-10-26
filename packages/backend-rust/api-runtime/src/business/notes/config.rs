//! Immutable public URL settings for native note/user/file packing.
use serde::Deserialize;

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    pub url: String,
    pub host: String,
    pub media_proxy: String,
    pub external_media_proxy_enabled: bool,
    pub video_thumbnail_generator: Option<String>,
    #[serde(skip)]
    pub emoji_cache: std::sync::Arc<tokio::sync::Mutex<super::entities::EmojiCache>>,
}
