//! `cli gallery-dl`: import files downloaded by gallery-dl.
//!
//! Metadata comes from the `<file>.<ext>.json` sidecars gallery-dl writes with
//! `--write-metadata` (default), or from a `gallery-dl -J` dump. Each image is
//! stored through the shared import path (`crate::import`).

mod metadata;

use std::collections::HashMap;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use fulgorart_db::Db;
use fulgorart_storage::R2Client;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::import::{self, ImageStore, ImportImage, ImportOutcome};

const MAX_CONCURRENT_UPLOADS: usize = 8;
/// Extensions imported as images; anything else (zip, mp4, ...) is skipped.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp"];

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Directory containing files downloaded by gallery-dl (searched recursively)
    pub image_dir: PathBuf,
    /// Metadata source. Omit to read the `<file>.<ext>.json` sidecars written by
    /// `gallery-dl --write-metadata`. Otherwise a `gallery-dl -J` dump (its
    /// type-3 entries are used), or `-` to read that dump from stdin
    pub json_file: Option<PathBuf>,
    /// Delete each file (and its sidecar) once it is stored or found to be a
    /// duplicate, so only files that need attention stay in the directory
    #[arg(long)]
    pub remove_imported: bool,
}

/// One file to import and what its metadata says.
struct WorkItem {
    path: PathBuf,
    sidecar: Option<PathBuf>,
    entry: metadata::Entry,
}

#[derive(Debug, Default)]
struct Summary {
    uploaded: usize,
    duplicates: usize,
    rejected: usize,
    failed: usize,
    no_metadata: usize,
    skipped_non_image: usize,
}

pub async fn run(args: Args, db: &Db, r2: &R2Client) -> Result<()> {
    let mut summary = Summary::default();
    let items = match &args.json_file {
        None => items_from_sidecars(&args.image_dir, &mut summary)?,
        Some(path) => {
            let json = if path.as_os_str() == "-" {
                let mut input = String::new();
                io::stdin().read_to_string(&mut input)?;
                input
            } else {
                std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read JSON file '{}'", path.display()))?
            };
            items_from_dump(&args.image_dir, &json, &mut summary)?
        }
    };

    let store: Arc<dyn ImageStore> = Arc::new(r2.clone());
    let slots = Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS));
    let mut tasks = JoinSet::new();
    for item in items {
        let permit = slots.clone().acquire_owned().await?;
        let (db, store, remove) = (db.clone(), store.clone(), args.remove_imported);
        tasks.spawn(async move {
            let _permit = permit;
            let result = import_file(&db, store.as_ref(), &item).await;
            if remove && matches!(result, Ok(ImportOutcome::Uploaded { .. } | ImportOutcome::Duplicate { .. })) {
                remove_file(&item.path);
                if let Some(sidecar) = &item.sidecar {
                    remove_file(sidecar);
                }
            }
            (item.path, result)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        let (path, result) = joined.context("import task panicked")?;
        match result {
            Ok(ImportOutcome::Uploaded { image_id, key }) => {
                summary.uploaded += 1;
                println!("uploaded path={} image_id={image_id} key={key}", path.display());
            }
            Ok(ImportOutcome::Duplicate { sha256 }) => {
                summary.duplicates += 1;
                println!("skipped_duplicate path={} sha256={sha256}", path.display());
            }
            Ok(ImportOutcome::Rejected(reason)) => {
                summary.rejected += 1;
                eprintln!("rejected path={} reason={reason}", path.display());
            }
            Err(error) => {
                summary.failed += 1;
                eprintln!("failed path={} error={error:#}", path.display());
            }
        }
    }

    println!(
        "uploaded={} duplicates={} rejected={} failed={} no_metadata={} skipped_non_image={}",
        summary.uploaded,
        summary.duplicates,
        summary.rejected,
        summary.failed,
        summary.no_metadata,
        summary.skipped_non_image
    );
    if summary.rejected + summary.failed + summary.no_metadata > 0 {
        bail!(
            "{} file(s) not imported; they were left in place (failed uploads are retried on the next run)",
            summary.rejected + summary.failed + summary.no_metadata
        );
    }
    Ok(())
}

async fn import_file(db: &Db, store: &dyn ImageStore, item: &WorkItem) -> Result<ImportOutcome> {
    let data = tokio::fs::read(&item.path)
        .await
        .with_context(|| format!("read {}", item.path.display()))?;
    let filename = item
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .context("file name is not UTF-8")?
        .to_owned();
    import::import_image(
        db,
        store,
        &item.entry.post,
        ImportImage {
            data,
            filename,
            source_url: item.entry.source_url.clone(),
        },
    )
    .await
}

fn remove_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        eprintln!("could not remove {}: {error}", path.display());
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Every file under `dir`, recursively.
fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("Failed to read directory '{}'", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Sidecar mode: each image `x.png` needs `x.png.json` (gallery-dl's
/// `--write-metadata` naming) or `x.json` next to it.
fn items_from_sidecars(dir: &Path, summary: &mut Summary) -> Result<Vec<WorkItem>> {
    let mut items = Vec::new();
    for path in walk(dir)? {
        if path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        if !is_image(&path) {
            summary.skipped_non_image += 1;
            continue;
        }
        let mut with_ext = path.clone().into_os_string();
        with_ext.push(".json");
        let candidates = [PathBuf::from(with_ext), path.with_extension("json")];
        let Some(sidecar) = candidates.into_iter().find(|p| p.is_file()) else {
            summary.no_metadata += 1;
            eprintln!("no_metadata path={}", path.display());
            continue;
        };
        let parsed = std::fs::read_to_string(&sidecar)
            .map_err(anyhow::Error::from)
            .and_then(|text| Ok(serde_json::from_str::<Value>(&text)?))
            .and_then(|kw| metadata::parse(&kw));
        match parsed {
            Ok(entry) => items.push(WorkItem {
                path,
                sidecar: Some(sidecar),
                entry,
            }),
            Err(error) => {
                summary.no_metadata += 1;
                eprintln!("bad_metadata path={} error={error:#}", sidecar.display());
            }
        }
    }
    Ok(items)
}

/// Dump mode: `gallery-dl -J` output; its type-3 entries `[3, url, kwdict]`
/// are matched to files in `dir` by name. Entries without a file (e.g.
/// already imported and removed) are ignored.
fn items_from_dump(dir: &Path, json: &str, summary: &mut Summary) -> Result<Vec<WorkItem>> {
    let messages: Vec<Value> = serde_json::from_str(json).context("Failed to parse gallery-dl JSON")?;
    let files: HashMap<String, PathBuf> = walk(dir)?
        .into_iter()
        .filter_map(|p| Some((p.file_name()?.to_str()?.to_owned(), p)))
        .collect();

    let mut items = Vec::new();
    for message in &messages {
        let Some([kind, url, kwdict]) = message.as_array().map(Vec::as_slice) else {
            continue;
        };
        if kind.as_i64() != Some(3) {
            continue;
        }
        let (Some(stem), Some(ext)) = (metadata::disk_stem(kwdict), kwdict.get("extension").and_then(Value::as_str)) else {
            continue;
        };
        let Some(path) = files.get(&format!("{stem}.{ext}")) else {
            continue;
        };
        if !is_image(path) {
            summary.skipped_non_image += 1;
            continue;
        }
        match metadata::parse(kwdict) {
            Ok(mut entry) => {
                entry.source_url = entry.source_url.or_else(|| url.as_str().map(str::to_owned));
                items.push(WorkItem {
                    path: path.clone(),
                    sidecar: None,
                    entry,
                });
            }
            Err(error) => {
                summary.no_metadata += 1;
                eprintln!("bad_metadata file={} error={error:#}", path.display());
            }
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fulgorart-gdl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn kwdict(id: u64) -> Value {
        json!({"category": "pixiv", "id": id, "filename": format!("{id}-abc_p0"),
               "hash": "abc", "extension": "png", "url": format!("https://i.pximg.net/{id}_p0.png")})
    }

    #[test]
    fn sidecar_mode_pairs_images_with_metadata() {
        let dir = temp_dir("sidecar");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("1_p0.png"), b"img").unwrap();
        std::fs::write(dir.join("1_p0.png.json"), kwdict(1).to_string()).unwrap();
        std::fs::write(dir.join("sub/2_p0.png"), b"img").unwrap();
        std::fs::write(dir.join("sub/2_p0.json"), kwdict(2).to_string()).unwrap(); // `x.json` naming
        std::fs::write(dir.join("3_p0.png"), b"img").unwrap(); // no sidecar
        std::fs::write(dir.join("4.zip"), b"zip").unwrap();
        std::fs::write(dir.join("5_p0.png"), b"img").unwrap();
        std::fs::write(dir.join("5_p0.png.json"), "{\"id\": 5}").unwrap(); // no category

        let mut summary = Summary::default();
        let items = items_from_sidecars(&dir, &mut summary).unwrap();
        let ids: Vec<&str> = items.iter().map(|i| i.entry.post.source_post_id.as_str()).collect();
        assert_eq!(ids, ["1", "2"]);
        assert!(items.iter().all(|i| i.sidecar.is_some()));
        assert_eq!((summary.no_metadata, summary.skipped_non_image), (2, 1));
    }

    #[test]
    fn dump_mode_matches_type3_entries_to_files() {
        let dir = temp_dir("dump");
        std::fs::write(dir.join("1_p0.png"), b"img").unwrap();
        let dump = json!([
            [2, {"category": "pixiv", "id": 1}],
            [3, "https://i.pximg.net/1_p0.png", kwdict(1)],
            [3, "https://i.pximg.net/9_p0.png", kwdict(9)] // not downloaded
        ]);
        let mut summary = Summary::default();
        let items = items_from_dump(&dir, &dump.to_string(), &mut summary).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, dir.join("1_p0.png"));
        assert!(items[0].sidecar.is_none());
    }
}
