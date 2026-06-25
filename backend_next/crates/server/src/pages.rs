use crate::response::ApiError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

const NOW: &str = "2026-06-06T00:00:00Z";
const DEFAULT_MARKDOWN: &str = "# Sub2API\n\nThis markdown page is served by backend_next.";

pub struct PageService {
    pages: RwLock<HashMap<String, PageRecord>>,
    pages_dir: PathBuf,
}

pub struct PageImage {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PageRecord {
    slug: String,
    title: String,
    content: String,
    visibility: String,
    created_at: String,
    updated_at: String,
}

impl PageService {
    pub fn new() -> Self {
        Self::with_pages_dir(default_pages_dir())
    }

    pub fn with_pages_dir(pages_dir: impl Into<PathBuf>) -> Self {
        let mut pages = HashMap::new();
        pages.insert(
            "welcome".to_owned(),
            PageRecord {
                slug: "welcome".to_owned(),
                title: "Welcome".to_owned(),
                content: DEFAULT_MARKDOWN.to_owned(),
                visibility: "user".to_owned(),
                created_at: NOW.to_owned(),
                updated_at: NOW.to_owned(),
            },
        );
        Self {
            pages: RwLock::new(pages),
            pages_dir: pages_dir.into(),
        }
    }

    pub fn get_markdown(&self, slug: &str) -> Result<String, ApiError> {
        validate_slug(slug)?;
        self.pages
            .read()
            .expect("pages lock")
            .get(slug)
            .filter(|page| page.visibility != "disabled")
            .map(|page| page.content.clone())
            .ok_or_else(|| ApiError::not_found("page not found"))
    }

    pub fn list(&self) -> Value {
        let mut pages = self
            .pages
            .read()
            .expect("pages lock")
            .values()
            .map(PageRecord::public_json)
            .collect::<Vec<_>>();
        pages.sort_by(|left, right| {
            left["slug"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["slug"].as_str().unwrap_or_default())
        });
        json!(pages)
    }

    pub fn upsert(&self, slug: &str, payload: Value) -> Result<Value, ApiError> {
        validate_slug(slug)?;
        let mut pages = self.pages.write().expect("pages lock");
        let page = pages.entry(slug.to_owned()).or_insert_with(|| PageRecord {
            slug: slug.to_owned(),
            title: slug.to_owned(),
            content: String::new(),
            visibility: "user".to_owned(),
            created_at: NOW.to_owned(),
            updated_at: NOW.to_owned(),
        });

        if let Some(title) = payload.get("title").and_then(Value::as_str) {
            page.title = title.to_owned();
        }
        if let Some(content) = payload
            .get("content")
            .or_else(|| payload.get("markdown"))
            .and_then(Value::as_str)
        {
            page.content = content.to_owned();
        }
        if let Some(visibility) = payload.get("visibility").and_then(Value::as_str) {
            page.visibility = visibility.to_owned();
        }
        page.updated_at = NOW.to_owned();
        Ok(page.public_json())
    }

    pub fn create(&self, payload: Value) -> Result<Value, ApiError> {
        let slug = payload
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_request("slug is required"))?
            .to_owned();
        self.upsert(&slug, payload)
    }

    pub fn delete(&self, slug: &str) -> Result<Value, ApiError> {
        validate_slug(slug)?;
        self.pages.write().expect("pages lock").remove(slug);
        Ok(json!({ "message": "deleted" }))
    }

    pub fn get_image(&self, slug: &str, filename: &str) -> Result<PageImage, ApiError> {
        validate_slug(slug).map_err(|_| ApiError::not_found("page image not found"))?;
        if !self.is_image_visible(slug) {
            return Err(ApiError::not_found("page image not found"));
        }
        let relative = clean_image_relative_path(filename)
            .ok_or_else(|| ApiError::not_found("page image not found"))?;
        let image_dir = self.pages_dir.join(slug);
        let target = resolve_image_path(&self.pages_dir, &image_dir, &relative)
            .ok_or_else(|| ApiError::not_found("page image not found"))?;
        let metadata =
            fs::metadata(&target).map_err(|_| ApiError::not_found("page image not found"))?;
        if metadata.is_dir() {
            return Err(ApiError::not_found("page image not found"));
        }
        let bytes = fs::read(&target)
            .map_err(|_| ApiError::internal_server_error("failed to read page image"))?;
        Ok(PageImage {
            content_type: image_content_type(&target),
            bytes,
        })
    }

    fn is_image_visible(&self, slug: &str) -> bool {
        self.pages
            .read()
            .expect("pages lock")
            .get(slug)
            .is_some_and(|page| page.visibility != "admin" && page.visibility != "disabled")
    }
}

impl PageRecord {
    fn public_json(&self) -> Value {
        json!({
            "slug": self.slug,
            "title": self.title,
            "content": self.content,
            "visibility": self.visibility,
            "created_at": self.created_at,
            "updated_at": self.updated_at
        })
    }
}

fn validate_slug(slug: &str) -> Result<(), ApiError> {
    let valid = !slug.is_empty()
        && slug.len() <= 64
        && slug.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_alphanumeric() || (index > 0 && (ch == '_' || ch == '-'))
        })
        && slug
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid page slug"))
    }
}

fn default_pages_dir() -> PathBuf {
    env::var("DATA_DIR")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"))
        .join("pages")
}

fn clean_image_relative_path(filename: &str) -> Option<PathBuf> {
    let filename = filename.trim_start_matches('/');
    if filename.is_empty()
        || filename.starts_with('/')
        || filename.contains('\\')
        || filename.contains('\0')
    {
        return None;
    }
    let decoded = percent_decode_path(filename)?;
    if decoded.is_empty()
        || decoded.starts_with('/')
        || decoded.contains('\\')
        || decoded.contains('\0')
    {
        return None;
    }
    let mut relative = PathBuf::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            value => relative.push(value),
        }
    }
    if relative.as_os_str().is_empty() || relative.is_absolute() || has_windows_prefix(&relative) {
        return None;
    }
    Some(relative)
}

fn percent_decode_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            output.push(from_hex_pair(hi, lo)?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn from_hex_pair(hi: u8, lo: u8) -> Option<u8> {
    Some(from_hex_digit(hi)? * 16 + from_hex_digit(lo)?)
}

fn from_hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(windows)]
fn has_windows_prefix(path: &Path) -> bool {
    use std::path::Component;
    path.components()
        .any(|component| matches!(component, Component::Prefix(_)))
}

#[cfg(not(windows))]
fn has_windows_prefix(_path: &Path) -> bool {
    false
}

fn resolve_image_path(pages_dir: &Path, image_dir: &Path, relative: &Path) -> Option<PathBuf> {
    let real_pages_dir = fs::canonicalize(pages_dir).ok()?;
    let real_image_dir = fs::canonicalize(image_dir).ok()?;
    if !path_is_within(&real_image_dir, &real_pages_dir) {
        return None;
    }
    let real_target = fs::canonicalize(image_dir.join(relative)).ok()?;
    if !path_is_within(&real_target, &real_image_dir) {
        return None;
    }
    Some(real_target)
}

fn path_is_within(path: &Path, base: &Path) -> bool {
    path.strip_prefix(base)
        .ok()
        .is_some_and(|relative| !relative.as_os_str().is_empty())
}

fn image_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_like_slugs() {
        assert!(validate_slug("../secret").is_err());
        assert!(validate_slug("valid-slug_1").is_ok());
    }

    #[test]
    fn rejects_unsafe_image_paths() {
        assert!(clean_image_relative_path("../secret.png").is_none());
        assert!(clean_image_relative_path("%2e%2e/secret.png").is_none());
        assert!(clean_image_relative_path("nested/logo.png").is_some());
        assert!(clean_image_relative_path("nested\\logo.png").is_none());
    }
}
