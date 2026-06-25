use crate::response::ApiError;
use flate2::read::GzDecoder;
use http::Uri;
use reqwest::Url;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_DOWNLOAD_SIZE: u64 = 500 * 1024 * 1024;
const MAX_BINARY_SIZE: u64 = 500 * 1024 * 1024;
const DEFAULT_ALLOWED_DOWNLOAD_HOSTS: &[&str] = &["github.com", "objects.githubusercontent.com"];

#[derive(Debug, Clone)]
pub struct SystemUpdateExecutor {
    executable_path: PathBuf,
    work_dir: PathBuf,
    allowed_hosts: Vec<String>,
    apply_replacement: bool,
}

impl SystemUpdateExecutor {
    pub fn current_process() -> Result<Self, ApiError> {
        let executable_path = std::env::current_exe().map_err(|error| {
            ApiError::internal_server_error(format!("failed to get executable path: {error}"))
        })?;
        let executable_path = fs::canonicalize(&executable_path).unwrap_or(executable_path);
        let work_dir = executable_path
            .parent()
            .ok_or_else(|| ApiError::internal_server_error("executable has no parent directory"))?
            .to_path_buf();
        Ok(Self {
            executable_path,
            work_dir,
            allowed_hosts: DEFAULT_ALLOWED_DOWNLOAD_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .collect(),
            apply_replacement: std::env::var("BACKEND_NEXT_UPDATE_APPLY").ok().as_deref()
                == Some("true"),
        })
    }

    #[cfg(test)]
    pub fn for_tests(
        executable_path: PathBuf,
        work_dir: PathBuf,
        allowed_hosts: Vec<String>,
        apply_replacement: bool,
    ) -> Self {
        Self {
            executable_path,
            work_dir,
            allowed_hosts,
            apply_replacement,
        }
    }

    pub async fn perform_update(&self, update_info: &Value) -> Result<Value, ApiError> {
        let plan = update_plan_from_info(update_info, &self.allowed_hosts)?;
        let temp_dir = create_temp_dir(&self.work_dir)?;
        let archive_path = temp_dir.join(file_name_from_url(&plan.download_url)?);
        let checksum_path = plan.checksum_url.as_ref().map(|url| {
            temp_dir.join(file_name_from_url(url).unwrap_or_else(|_| "checksums.txt".to_owned()))
        });

        let result = async {
            download_file(&plan.download_url, &archive_path, MAX_DOWNLOAD_SIZE).await?;
            if let (Some(checksum_url), Some(checksum_path)) =
                (&plan.checksum_url, checksum_path.as_ref())
            {
                download_file(checksum_url, checksum_path, 1024 * 1024).await?;
                verify_checksum(&archive_path, checksum_path)?;
            }
            let extracted_binary = extract_binary(&archive_path, &temp_dir)?;
            if self.apply_replacement {
                replace_executable(&self.executable_path, &extracted_binary)?;
            }
            Ok::<Value, ApiError>(json!({
                "success": true,
                "message": if self.apply_replacement { "Update applied" } else { "Update downloaded and verified" },
                "need_restart": self.apply_replacement,
                "status": "completed",
                "archive_path": archive_path.to_string_lossy(),
                "download_url": plan.download_url,
                "checksum_url": plan.checksum_url,
                "asset_name": plan.asset_name,
                "binary_path": extracted_binary.to_string_lossy(),
                "applied": self.apply_replacement
            }))
        }
        .await;

        if result.is_ok() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        result
    }

    pub fn rollback(&self) -> Result<Value, ApiError> {
        let backup = backup_path_for(&self.executable_path);
        if !backup.exists() {
            return Err(ApiError::internal_server_error("no backup found"));
        }
        if self.executable_path.exists() {
            fs::remove_file(&self.executable_path).map_err(|error| {
                ApiError::internal_server_error(format!(
                    "failed to remove current binary before rollback: {error}"
                ))
            })?;
        }
        fs::rename(&backup, &self.executable_path).map_err(|error| {
            ApiError::internal_server_error(format!("rollback failed: {error}"))
        })?;
        Ok(json!({
            "success": true,
            "message": "Rollback completed",
            "need_restart": true,
            "status": "completed",
            "restored_path": self.executable_path.to_string_lossy()
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdatePlan {
    asset_name: String,
    download_url: String,
    checksum_url: Option<String>,
}

fn update_plan_from_info(
    update_info: &Value,
    allowed_hosts: &[String],
) -> Result<UpdatePlan, ApiError> {
    let assets = update_info
        .get("release_info")
        .and_then(|release_info| release_info.get("assets"))
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request("release assets are missing"))?;
    let archive_marker = archive_marker();
    let mut checksum_url = None;
    let mut selected = None;

    for asset in assets {
        let name = asset
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let download_url = asset
            .get("download_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if name == "checksums.txt" {
            if !download_url.is_empty() {
                validate_download_url(download_url, allowed_hosts)?;
                checksum_url = Some(download_url.to_owned());
            }
            continue;
        }
        if selected.is_none()
            && !name.ends_with(".txt")
            && name.contains(&archive_marker)
            && !download_url.is_empty()
        {
            validate_download_url(download_url, allowed_hosts)?;
            selected = Some((name.to_owned(), download_url.to_owned()));
        }
    }

    let (asset_name, download_url) = selected.ok_or_else(|| {
        ApiError::internal_server_error(format!(
            "no compatible release found for {}",
            archive_marker
        ))
    })?;
    Ok(UpdatePlan {
        asset_name,
        download_url,
        checksum_url,
    })
}

fn archive_marker() -> String {
    format!("{}_{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn validate_download_url(raw_url: &str, allowed_hosts: &[String]) -> Result<(), ApiError> {
    let parsed = Url::parse(raw_url)
        .map_err(|error| ApiError::bad_request(format!("invalid update download URL: {error}")))?;
    if parsed.scheme() != "https" && !is_loopback_test_url(&parsed) {
        return Err(ApiError::bad_request("only HTTPS update URLs are allowed"));
    }
    if is_loopback_test_url(&parsed) {
        return Ok(());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ApiError::bad_request("update download URL is missing host"))?;
    let allowed = allowed_hosts.iter().any(|allowed| {
        host.eq_ignore_ascii_case(allowed)
            || host
                .to_ascii_lowercase()
                .ends_with(&format!(".{}", allowed.to_ascii_lowercase()))
    });
    if !allowed {
        return Err(ApiError::bad_request(format!(
            "download from untrusted host: {host}"
        )));
    }
    Ok(())
}

fn is_loopback_test_url(parsed: &Url) -> bool {
    parsed
        .host_str()
        .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1"))
        && cfg!(test)
}

async fn download_file(url: &str, path: &Path, max_size: u64) -> Result<(), ApiError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent("sub2api-backend-next")
        .build()
        .map_err(|error| {
            ApiError::internal_server_error(format!("failed to build update HTTP client: {error}"))
        })?;
    let response = client.get(url).send().await.map_err(|error| {
        ApiError::internal_server_error(format!("failed to download update asset: {error}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::internal_server_error(format!(
            "failed to download update asset: HTTP {status}"
        )));
    }
    let bytes = response.bytes().await.map_err(|error| {
        ApiError::internal_server_error(format!("failed to read update asset: {error}"))
    })?;
    if bytes.len() as u64 > max_size {
        return Err(ApiError::bad_request("update asset is too large"));
    }
    fs::write(path, &bytes).map_err(|error| {
        ApiError::internal_server_error(format!("failed to write update asset: {error}"))
    })
}

fn verify_checksum(archive_path: &Path, checksum_path: &Path) -> Result<(), ApiError> {
    let expected_name = archive_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| ApiError::internal_server_error("archive path has no filename"))?;
    let actual = sha256_file(archive_path)?;
    let file = fs::File::open(checksum_path).map_err(|error| {
        ApiError::internal_server_error(format!("failed to open checksums file: {error}"))
    })?;
    for line in io::BufReader::new(file).lines() {
        let line = line.map_err(|error| {
            ApiError::internal_server_error(format!("failed to read checksums file: {error}"))
        })?;
        let mut parts = line.split_whitespace();
        let Some(expected_hash) = parts.next() else {
            continue;
        };
        let Some(file_name) = parts.next() else {
            continue;
        };
        if file_name == expected_name {
            if expected_hash.eq_ignore_ascii_case(&actual) {
                return Ok(());
            }
            return Err(ApiError::bad_request(format!(
                "checksum mismatch for {expected_name}"
            )));
        }
    }
    Err(ApiError::bad_request(format!(
        "checksum not found for {expected_name}"
    )))
}

fn sha256_file(path: &Path) -> Result<String, ApiError> {
    let mut file = fs::File::open(path).map_err(|error| {
        ApiError::internal_server_error(format!("failed to open file for checksum: {error}"))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            ApiError::internal_server_error(format!("failed to read file for checksum: {error}"))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn extract_binary(archive_path: &Path, output_dir: &Path) -> Result<PathBuf, ApiError> {
    let output_path = output_dir.join(binary_name());
    let file = fs::File::open(archive_path).map_err(|error| {
        ApiError::internal_server_error(format!("failed to open update archive: {error}"))
    })?;
    let extension = archive_path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if extension.contains(".tar") {
        let reader: Box<dyn Read> = if extension.ends_with(".gz") || extension.ends_with(".tgz") {
            Box::new(GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let mut archive = tar::Archive::new(reader);
        for entry in archive.entries().map_err(|error| {
            ApiError::internal_server_error(format!("failed to read update archive: {error}"))
        })? {
            let mut entry = entry.map_err(|error| {
                ApiError::internal_server_error(format!("failed to read archive entry: {error}"))
            })?;
            let path = entry.path().map_err(|error| {
                ApiError::internal_server_error(format!(
                    "failed to read archive entry path: {error}"
                ))
            })?;
            let Some(base_name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if path
                .components()
                .any(|component| component.as_os_str() == "..")
            {
                return Err(ApiError::bad_request(
                    "path traversal attempt in update archive",
                ));
            }
            if base_name != "sub2api" && base_name != "sub2api.exe" {
                continue;
            }
            if entry.header().size().unwrap_or(MAX_BINARY_SIZE + 1) > MAX_BINARY_SIZE {
                return Err(ApiError::bad_request("update binary is too large"));
            }
            entry.unpack(&output_path).map_err(|error| {
                ApiError::internal_server_error(format!("failed to extract update binary: {error}"))
            })?;
            return Ok(output_path);
        }
        return Err(ApiError::bad_request("binary not found in update archive"));
    }

    let metadata = fs::metadata(archive_path).map_err(|error| {
        ApiError::internal_server_error(format!("failed to inspect update asset: {error}"))
    })?;
    if metadata.len() > MAX_BINARY_SIZE {
        return Err(ApiError::bad_request("update binary is too large"));
    }
    fs::copy(archive_path, &output_path).map_err(|error| {
        ApiError::internal_server_error(format!("failed to copy update binary: {error}"))
    })?;
    Ok(output_path)
}

fn replace_executable(current: &Path, replacement: &Path) -> Result<(), ApiError> {
    let backup = backup_path_for(current);
    if backup.exists() {
        fs::remove_file(&backup).map_err(|error| {
            ApiError::internal_server_error(format!("failed to remove old backup binary: {error}"))
        })?;
    }
    fs::rename(current, &backup).map_err(|error| {
        ApiError::internal_server_error(format!("failed to backup current binary: {error}"))
    })?;
    if let Err(error) = fs::rename(replacement, current) {
        let _ = fs::rename(&backup, current);
        return Err(ApiError::internal_server_error(format!(
            "failed to replace binary: {error}"
        )));
    }
    Ok(())
}

fn backup_path_for(current: &Path) -> PathBuf {
    current.with_extension(format!(
        "{}backup",
        current
            .extension()
            .and_then(OsStr::to_str)
            .map(|extension| format!("{extension}."))
            .unwrap_or_default()
    ))
}

fn file_name_from_url(raw_url: &str) -> Result<String, ApiError> {
    let uri = raw_url
        .parse::<Uri>()
        .map_err(|_| ApiError::bad_request("invalid update asset URL"))?;
    uri.path()
        .rsplit('/')
        .next()
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ApiError::bad_request("update asset URL has no filename"))
}

fn create_temp_dir(parent: &Path) -> Result<PathBuf, ApiError> {
    let dir = parent.join(format!(".sub2api-update-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&dir).map_err(|error| {
        ApiError::internal_server_error(format!("failed to create update temp dir: {error}"))
    })?;
    Ok(dir)
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "sub2api.exe"
    } else {
        "sub2api"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tar::Builder;

    #[test]
    fn selects_platform_asset_and_checksum() {
        let marker = archive_marker();
        let info = json!({
            "release_info": {
                "assets": [
                    { "name": "checksums.txt", "download_url": "https://github.com/test/checksums.txt", "size": 10 },
                    { "name": format!("sub2api_{marker}.tar.gz"), "download_url": "https://github.com/test/sub2api.tar.gz", "size": 20 }
                ]
            }
        });
        let plan = update_plan_from_info(&info, &["github.com".to_owned()]).unwrap();
        assert_eq!(plan.asset_name, format!("sub2api_{marker}.tar.gz"));
        assert_eq!(
            plan.checksum_url.as_deref(),
            Some("https://github.com/test/checksums.txt")
        );
    }

    #[test]
    fn rejects_untrusted_download_host() {
        let marker = archive_marker();
        let info = json!({
            "release_info": {
                "assets": [
                    { "name": format!("sub2api_{marker}.tar.gz"), "download_url": "https://evil.example/sub2api.tar.gz", "size": 20 }
                ]
            }
        });
        assert!(update_plan_from_info(&info, &["github.com".to_owned()]).is_err());
    }

    #[test]
    fn extracts_binary_from_tar_gz() {
        let temp = std::env::temp_dir().join(format!(
            "sub2api-update-extract-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&temp).unwrap();
        let archive_path = temp.join("sub2api_test.tar.gz");
        let archive = fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(archive, Compression::default());
        let mut tar = Builder::new(encoder);
        let content = b"new-binary";
        let mut header = tar::Header::new_gnu();
        header.set_path(binary_name()).unwrap();
        header.set_size(content.len() as u64);
        header.set_cksum();
        tar.append(&header, &content[..]).unwrap();
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();

        let extracted = extract_binary(&archive_path, &temp).unwrap();
        assert_eq!(fs::read(extracted).unwrap(), content);
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn verifies_checksum_file() {
        let temp = std::env::temp_dir().join(format!(
            "sub2api-update-checksum-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&temp).unwrap();
        let archive = temp.join("archive.tar.gz");
        fs::write(&archive, b"archive").unwrap();
        let digest = sha256_file(&archive).unwrap();
        let checksums = temp.join("checksums.txt");
        let mut file = fs::File::create(&checksums).unwrap();
        writeln!(file, "{digest}  archive.tar.gz").unwrap();
        verify_checksum(&archive, &checksums).unwrap();
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn replace_and_rollback_use_backup_file() {
        let temp = std::env::temp_dir().join(format!(
            "sub2api-update-rollback-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&temp).unwrap();
        let current = temp.join(binary_name());
        let replacement = temp.join("replacement");
        fs::write(&current, b"old").unwrap();
        fs::write(&replacement, b"new").unwrap();

        replace_executable(&current, &replacement).unwrap();
        assert_eq!(fs::read(&current).unwrap(), b"new");
        assert_eq!(fs::read(backup_path_for(&current)).unwrap(), b"old");

        let executor = SystemUpdateExecutor::for_tests(
            current.clone(),
            temp.clone(),
            vec!["localhost".to_owned()],
            true,
        );
        let rollback = executor.rollback().unwrap();
        assert_eq!(rollback["success"], true);
        assert_eq!(fs::read(&current).unwrap(), b"old");
        let _ = fs::remove_dir_all(temp);
    }
}
