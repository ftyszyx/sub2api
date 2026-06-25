use crate::backup_executor::{dump_postgres_gzip, PostgresDumpConfig};
use crate::filesystem_dump::{collect_filesystem_dump, FilesystemDump, FilesystemDumpConfig};
use crate::redis_dump::{dump_redis, RedisDumpConfig};
use crate::response::ApiError;
use crate::s3_probe::{self, S3ProbeConfig, S3PutObjectInput};
use chrono::Utc;
use flate2::{write::GzEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt::Write as FmtWrite;
use std::io::Write;

const ARTIFACT_CONTENT_TYPE: &str = "application/gzip";

#[derive(Debug, Clone)]
pub struct DataManagementExecutionInput {
    pub job: Value,
    pub upload_to_s3: bool,
    pub s3: Option<S3ProbeConfig>,
    pub redis_dump: Option<RedisDumpConfig>,
    pub filesystem_dump: Option<FilesystemDumpConfig>,
    pub postgres_dump: Option<PostgresDumpConfig>,
}

pub async fn execute_data_management_backup_job(
    input: DataManagementExecutionInput,
) -> Result<Value, ApiError> {
    let redis_dump = input
        .redis_dump
        .as_ref()
        .map(dump_redis)
        .transpose()
        .map_err(|error| ApiError::bad_request(format!("Redis dump failed: {error}")))?;
    let filesystem_dump = input
        .filesystem_dump
        .as_ref()
        .map(collect_filesystem_dump)
        .transpose()
        .map_err(|error| ApiError::bad_request(format!("Filesystem dump failed: {error}")))?;
    let postgres_dump = if let Some(config) = input.postgres_dump.as_ref() {
        let config = config.clone();
        Some(
            tokio::task::spawn_blocking(move || dump_postgres_gzip(&config))
                .await
                .map_err(|error| {
                    ApiError::internal_server_error(format!("Postgres dump task failed: {error}"))
                })?
                .map_err(|error| {
                    ApiError::bad_request(format!("Postgres dump failed: {}", error.message()))
                })?,
        )
    } else {
        None
    };
    let artifact = build_artifact(
        &input.job,
        redis_dump.as_ref(),
        filesystem_dump.as_ref(),
        postgres_dump.as_deref(),
    )?;
    let mut job = input.job;
    let finished_at = Utc::now().to_rfc3339();
    job["status"] = json!("completed");
    job["finished_at"] = json!(finished_at);
    job["error_message"] = Value::Null;
    job["artifact"]["size_bytes"] = json!(artifact.len());
    job["artifact"]["sha256"] = json!(sha256_hex(&artifact));
    job["artifact"]["content_type"] = json!(ARTIFACT_CONTENT_TYPE);
    job["execution_plan"]["executor"] = json!("repository-artifact");
    job["execution_plan"]["requires_executor"] = json!(false);
    if let Some(redis_dump) = redis_dump.as_ref() {
        job["artifact"]["redis"] = json!({
            "included": true,
            "key_count": redis_dump.manifest.get("key_count").cloned().unwrap_or_else(|| json!(0)),
            "format": "jsonl"
        });
    }
    if let Some(filesystem_dump) = filesystem_dump.as_ref() {
        job["artifact"]["filesystem"] = json!({
            "included": true,
            "file_count": filesystem_dump.files.len(),
            "total_bytes": filesystem_dump.manifest.get("total_bytes").cloned().unwrap_or_else(|| json!(0))
        });
    }
    if let Some(postgres_dump) = postgres_dump.as_ref() {
        job["artifact"]["postgres"] = json!({
            "included": true,
            "size_bytes": postgres_dump.len(),
            "format": "sql.gz"
        });
    }
    write_local_artifact(&job, &artifact)?;

    if input.upload_to_s3 {
        let s3 = input.s3.as_ref().ok_or_else(|| {
            ApiError::bad_request("S3 profile is required when upload_to_s3 is true")
        })?;
        let key = job
            .get("s3")
            .and_then(|s3| s3.get("key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::bad_request("data-management S3 key is required"))?;
        let size = s3_probe::put_object(
            s3,
            S3PutObjectInput {
                key: key.to_owned(),
                body: artifact,
                content_type: ARTIFACT_CONTENT_TYPE.to_owned(),
            },
        )
        .await?;
        job["s3"]["size_bytes"] = json!(size);
        job["s3"]["content_type"] = json!(ARTIFACT_CONTENT_TYPE);
    }

    Ok(job)
}

fn write_local_artifact(job: &Value, artifact: &[u8]) -> Result<(), ApiError> {
    let Some(local_path) = job
        .get("artifact")
        .and_then(|artifact| artifact.get("local_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let path = std::path::Path::new(local_path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            ApiError::internal_server_error(format!(
                "data-management artifact directory creation failed: {error}"
            ))
        })?;
    }
    std::fs::write(path, artifact).map_err(|error| {
        ApiError::internal_server_error(format!(
            "data-management local artifact write failed: {error}"
        ))
    })?;
    Ok(())
}

fn build_artifact(
    job: &Value,
    redis_dump: Option<&crate::redis_dump::RedisDump>,
    filesystem_dump: Option<&FilesystemDump>,
    postgres_dump: Option<&[u8]>,
) -> Result<Vec<u8>, ApiError> {
    let manifest = json!({
        "version": 1,
        "kind": "sub2api.data-management-backup",
        "created_at": Utc::now().to_rfc3339(),
        "job": job,
        "sources": {
            "redis": redis_dump.map(|dump| dump.manifest.clone()),
            "filesystem": filesystem_dump.map(|dump| dump.manifest.clone()),
            "postgres": postgres_dump.map(|dump| json!({
                "kind": "sub2api.postgres-dump",
                "version": 1,
                "created_at": Utc::now().to_rfc3339(),
                "format": "sql.gz",
                "size_bytes": dump.len()
            }))
        }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| ApiError::internal_server_error(error.to_string()))?;
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        append_tar_file(&mut builder, "manifest.json", &manifest_bytes)?;
        if let Some(redis_dump) = redis_dump {
            append_tar_file(&mut builder, "redis/dump.jsonl", &redis_dump.jsonl)?;
        }
        if let Some(postgres_dump) = postgres_dump {
            append_tar_file(&mut builder, "postgres/dump.sql.gz", postgres_dump)?;
        }
        if let Some(filesystem_dump) = filesystem_dump {
            for file in &filesystem_dump.files {
                let bytes = std::fs::read(&file.source_path).map_err(|error| {
                    ApiError::internal_server_error(format!(
                        "data-management asset read failed: {error}"
                    ))
                })?;
                append_tar_file(&mut builder, &file.archive_path, &bytes)?;
            }
        }
        builder.finish().map_err(|error| {
            ApiError::internal_server_error(format!("data-management artifact failed: {error}"))
        })?;
    }
    gzip_bytes(&tar_bytes)
}

fn append_tar_file<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<(), ApiError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .map_err(|error| {
            ApiError::internal_server_error(format!("data-management artifact failed: {error}"))
        })?;
    Ok(())
}

fn gzip_bytes(input: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).map_err(|error| {
        ApiError::internal_server_error(format!("data-management gzip failed: {error}"))
    })?;
    encoder.finish().map_err(|error| {
        ApiError::internal_server_error(format!("data-management gzip failed: {error}"))
    })
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    #[test]
    fn artifact_contains_job_manifest() {
        let artifact = build_artifact(
            &json!({
                "job_id": "job-test",
                "backup_type": "full"
            }),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(!artifact.is_empty());
        let mut gzip = GzDecoder::new(artifact.as_slice());
        let mut tar_bytes = Vec::new();
        gzip.read_to_end(&mut tar_bytes).unwrap();
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let mut manifest = String::new();
        archive
            .entries()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert!(manifest.contains("job-test"));
    }
}
