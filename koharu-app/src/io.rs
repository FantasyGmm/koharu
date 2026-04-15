use image::ImageFormat;
use koharu_core::Document;
use koharu_core::commands::{
    DeviceInfo, FileEntry, FileResult, OpenDocumentsPayload, OpenExternalPayload, ThumbnailResult,
};
use rfd::FileDialog;
use std::path::{Path, PathBuf};

use crate::AppResources;
use crate::utils::{encode_image_dynamic, mime_from_ext};

const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    Files,
    Folder,
}

pub async fn app_version(state: AppResources) -> anyhow::Result<String> {
    Ok(state.version.to_string())
}

pub async fn device(state: AppResources) -> anyhow::Result<DeviceInfo> {
    Ok(DeviceInfo {
        ml_device: match state.device {
            koharu_ml::Device::Cpu => "CPU".to_string(),
            koharu_ml::Device::Cuda(_) => "CUDA".to_string(),
            koharu_ml::Device::Metal(_) => "Metal".to_string(),
        },
    })
}

pub async fn open_external(
    _state: AppResources,
    payload: OpenExternalPayload,
) -> anyhow::Result<()> {
    open::that(&payload.url)?;
    Ok(())
}

pub async fn get_documents(state: AppResources) -> anyhow::Result<usize> {
    Ok(state.storage.page_count().await)
}

// list_documents is now async — callers use storage.list_pages() directly

pub async fn get_document(state: AppResources, document_id: &str) -> anyhow::Result<Document> {
    state.storage.page(document_id).await
}

pub async fn get_thumbnail(
    state: AppResources,
    document_id: &str,
) -> anyhow::Result<ThumbnailResult> {
    let doc = state.storage.page(document_id).await?;

    let source_ref = doc.rendered.as_ref().unwrap_or(&doc.source);
    let source_img = state.storage.images.load(source_ref)?;
    let thumbnail = source_img.thumbnail(200, 200);

    let mut buf = std::io::Cursor::new(Vec::new());
    thumbnail.write_to(&mut buf, ImageFormat::WebP)?;

    Ok(ThumbnailResult {
        data: buf.into_inner(),
        content_type: "image/webp".to_string(),
    })
}

#[tracing::instrument(level = "info", skip_all)]
pub async fn open_documents(
    state: AppResources,
    payload: OpenDocumentsPayload,
) -> anyhow::Result<usize> {
    if payload.files.is_empty() {
        anyhow::bail!("No files uploaded");
    }

    let pages = state.storage.import_files(payload.files, true).await?;
    Ok(pages.len())
}

#[tracing::instrument(level = "info", skip_all)]
pub async fn add_documents(
    state: AppResources,
    payload: OpenDocumentsPayload,
) -> anyhow::Result<usize> {
    if payload.files.is_empty() {
        anyhow::bail!("No files uploaded");
    }

    let _new_pages = state.storage.import_files(payload.files, false).await?;
    Ok(state.storage.page_count().await)
}

#[tracing::instrument(level = "info", skip_all)]
pub async fn import_documents_from_dialog(
    state: AppResources,
    replace: bool,
    source: ImportSource,
) -> anyhow::Result<usize> {
    let Some(files) = pick_import_entries(source).await? else {
        anyhow::bail!("__CANCELLED__");
    };

    if files.is_empty() {
        anyhow::bail!("No supported image files found");
    }

    let count = state.storage.import_files(files, replace).await?.len();
    if replace {
        Ok(count)
    } else {
        Ok(state.storage.page_count().await)
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub async fn export_document(state: AppResources, document_id: &str) -> anyhow::Result<FileResult> {
    let doc = state.storage.page(document_id).await?;

    let rendered_ref = doc
        .rendered
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No rendered image found"))?;
    let rendered_img = state.storage.images.load(rendered_ref)?;

    let ext = "webp";
    let bytes = encode_image_dynamic(&rendered_img, ext)?;
    let filename = format!("{}_koharu.{}", doc.name, ext);
    let content_type = mime_from_ext(ext).to_string();

    Ok(FileResult {
        filename,
        data: bytes,
        content_type,
    })
}

pub async fn export_all_inpainted(state: AppResources) -> anyhow::Result<usize> {
    let Some(output_dir) = pick_output_dir().await? else {
        return Ok(0);
    };

    let pages = state.storage.with_project(|p| p.pages.clone()).await;
    let mut exported = 0usize;
    for doc in &pages {
        let Some(ref inpainted_ref) = doc.inpainted else {
            continue;
        };
        let img = state.storage.images.load(inpainted_ref)?;
        let output_path = output_dir.join(format!("{}_inpainted.webp", doc.name));
        let bytes = encode_image_dynamic(&img, "webp")?;
        std::fs::write(&output_path, bytes)?;
        exported += 1;
    }
    anyhow::ensure!(exported > 0, "No inpainted images found to export");
    Ok(exported)
}

pub async fn export_all_rendered(state: AppResources) -> anyhow::Result<usize> {
    let Some(output_dir) = pick_output_dir().await? else {
        return Ok(0);
    };

    let pages = state.storage.with_project(|p| p.pages.clone()).await;
    let mut exported = 0usize;
    for doc in &pages {
        let Some(ref rendered_ref) = doc.rendered else {
            continue;
        };
        let img = state.storage.images.load(rendered_ref)?;
        let output_path = output_dir.join(format!("{}_rendered.webp", doc.name));
        let bytes = encode_image_dynamic(&img, "webp")?;
        std::fs::write(&output_path, bytes)?;
        exported += 1;
    }
    anyhow::ensure!(exported > 0, "No rendered images found to export");
    Ok(exported)
}

async fn pick_output_dir() -> anyhow::Result<Option<std::path::PathBuf>> {
    Ok(tokio::task::spawn_blocking(|| FileDialog::new().pick_folder()).await?)
}

async fn pick_import_entries(source: ImportSource) -> anyhow::Result<Option<Vec<FileEntry>>> {
    tokio::task::spawn_blocking(move || match source {
        ImportSource::Files => {
            let Some(paths) = FileDialog::new()
                .add_filter("Images", SUPPORTED_IMAGE_EXTENSIONS)
                .pick_files()
            else {
                return Ok(None);
            };
            Ok(Some(read_file_entries(paths)?))
        }
        ImportSource::Folder => {
            let Some(folder) = FileDialog::new().pick_folder() else {
                return Ok(None);
            };
            let mut paths = Vec::new();
            collect_supported_image_paths(&folder, &mut paths)?;
            paths.sort();
            Ok(Some(read_file_entries(paths)?))
        }
    })
    .await?
}

fn read_file_entries(paths: Vec<PathBuf>) -> anyhow::Result<Vec<FileEntry>> {
    paths
        .into_iter()
        .filter(|path| is_supported_image_path(path))
        .map(|path| {
            let data = std::fs::read(&path)
                .map_err(|err| anyhow::anyhow!("Failed to read {}: {err}", path.display()))?;
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            Ok(FileEntry { name, data })
        })
        .collect()
}

fn collect_supported_image_paths(root: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_supported_image_paths(&path, out)?;
        } else if file_type.is_file() && is_supported_image_path(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            SUPPORTED_IMAGE_EXTENSIONS
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
        .unwrap_or(false)
}
