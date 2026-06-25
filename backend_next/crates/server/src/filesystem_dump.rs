use anyhow::{anyhow, Context};
use chrono::Utc;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FilesystemDumpConfig {
    pub roots: Vec<PathBuf>,
    pub max_file_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct FilesystemDumpFile {
    pub archive_path: String,
    pub source_path: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct FilesystemDump {
    pub manifest: Value,
    pub files: Vec<FilesystemDumpFile>,
}

pub fn collect_filesystem_dump(config: &FilesystemDumpConfig) -> anyhow::Result<FilesystemDump> {
    let max_file_bytes = if config.max_file_bytes == 0 {
        25 * 1024 * 1024
    } else {
        config.max_file_bytes
    };
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for root in &config.roots {
        let root = root
            .canonicalize()
            .with_context(|| format!("failed to resolve asset path {}", root.display()))?;
        let meta = fs::metadata(&root)
            .with_context(|| format!("failed to inspect asset path {}", root.display()))?;
        if meta.is_file() {
            let Some(name) = root.file_name().and_then(|value| value.to_str()) else {
                skipped.push(json!({
                    "path": root.display().to_string(),
                    "reason": "missing file name"
                }));
                continue;
            };
            push_file(&mut files, &mut skipped, &root, name, max_file_bytes)?;
        } else if meta.is_dir() {
            let prefix = root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("assets")
                .to_owned();
            collect_dir(
                &root,
                &root,
                &prefix,
                max_file_bytes,
                &mut files,
                &mut skipped,
            )?;
        }
    }
    files.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    let total_bytes: u64 = files.iter().map(|file| file.size_bytes).sum();
    Ok(FilesystemDump {
        manifest: json!({
            "kind": "sub2api.filesystem-dump",
            "version": 1,
            "created_at": Utc::now().to_rfc3339(),
            "file_count": files.len(),
            "total_bytes": total_bytes,
            "max_file_bytes": max_file_bytes,
            "skipped": skipped
        }),
        files,
    })
}

fn collect_dir(
    root: &Path,
    current: &Path,
    prefix: &str,
    max_file_bytes: u64,
    files: &mut Vec<FilesystemDumpFile>,
    skipped: &mut Vec<Value>,
) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(current)
        .with_context(|| format!("failed to read asset directory {}", current.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let meta = entry
            .metadata()
            .with_context(|| format!("failed to inspect asset path {}", path.display()))?;
        if meta.is_dir() {
            collect_dir(root, &path, prefix, max_file_bytes, files, skipped)?;
        } else if meta.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| anyhow!("asset path escaped root"))?;
            let archive_path = format!(
                "{prefix}/{}",
                relative
                    .components()
                    .filter_map(|component| component.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/")
            );
            push_file(files, skipped, &path, &archive_path, max_file_bytes)?;
        }
    }
    Ok(())
}

fn push_file(
    files: &mut Vec<FilesystemDumpFile>,
    skipped: &mut Vec<Value>,
    path: &Path,
    archive_path: &str,
    max_file_bytes: u64,
) -> anyhow::Result<()> {
    let meta = fs::metadata(path)
        .with_context(|| format!("failed to inspect asset {}", path.display()))?;
    if meta.len() > max_file_bytes {
        skipped.push(json!({
            "path": path.display().to_string(),
            "reason": "file too large",
            "size_bytes": meta.len()
        }));
        return Ok(());
    }
    files.push(FilesystemDumpFile {
        archive_path: format!("filesystem/{}", sanitize_archive_path(archive_path)?),
        source_path: path.to_path_buf(),
        size_bytes: meta.len(),
    });
    Ok(())
}

fn sanitize_archive_path(path: &str) -> anyhow::Result<String> {
    let normalized = path.replace('\\', "/");
    let parts = normalized
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| *part == "." || *part == ".." || part.contains(':'))
    {
        return Err(anyhow!("unsafe archive path {path}"));
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn collects_directory_files_and_skips_large_files() {
        let root = temp_dir("filesystem-dump");
        fs::create_dir_all(root.join("public/nested")).unwrap();
        fs::write(root.join("public/index.html"), b"hello").unwrap();
        fs::write(root.join("public/nested/item.txt"), b"nested").unwrap();
        fs::write(root.join("public/large.bin"), b"too large").unwrap();

        let dump = collect_filesystem_dump(&FilesystemDumpConfig {
            roots: vec![root.join("public")],
            max_file_bytes: 6,
        })
        .unwrap();

        let paths = dump
            .files
            .iter()
            .map(|file| file.archive_path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                "filesystem/public/index.html",
                "filesystem/public/nested/item.txt"
            ]
        );
        assert_eq!(dump.manifest["file_count"], 2);
        assert_eq!(dump.manifest["skipped"].as_array().unwrap().len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join("backend_next_tests")
            .join(format!("{name}-{unique}"))
    }
}
