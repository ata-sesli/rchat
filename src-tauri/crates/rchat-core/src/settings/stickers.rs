use crate::{storage, AppState};
use anyhow::{anyhow, Context, Result};
use image::{codecs::webp::WebPEncoder, imageops::FilterType, DynamicImage, ExtendedColorType};
use std::path::Path;

const MAX_STICKER_SIZE_BYTES: usize = 1_000_000;
const MIN_STICKER_SIDE_PX: u32 = 96;
const MAX_STICKER_SCALE_ATTEMPTS: u32 = 16;

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct AddStickerResult {
    pub file_hash: String,
    pub name: String,
    pub converted: bool,
    pub already_exists: bool,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct StickerImportResult {
    pub file_path: String,
    pub file_hash: Option<String>,
    pub error: Option<String>,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct StickerBatchImportResult {
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<StickerImportResult>,
}

#[derive(Debug)]
struct PreparedSticker {
    file_name: String,
    file_data: Vec<u8>,
    converted: bool,
}

pub fn list_stickers(app_state: &AppState) -> Result<Vec<storage::db::Sticker>> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow!("failed to lock database while listing stickers: {error}")
    })?;
    storage::db::list_stickers(&conn)
}

pub fn add_sticker(app_state: &AppState, file_path: &str) -> Result<AddStickerResult> {
    let prepared = prepare_sticker_for_import(file_path)?;
    store_prepared_sticker(app_state, prepared, "local")
}

pub fn add_stickers_batch(
    app_state: &AppState,
    file_paths: Vec<String>,
) -> StickerBatchImportResult {
    let mut results = Vec::with_capacity(file_paths.len());
    let mut success_count = 0usize;
    let mut failure_count = 0usize;

    for file_path in file_paths {
        match add_sticker(app_state, &file_path) {
            Ok(result) => {
                success_count += 1;
                results.push(StickerImportResult {
                    file_path,
                    file_hash: Some(result.file_hash),
                    error: None,
                });
            }
            Err(error) => {
                failure_count += 1;
                results.push(StickerImportResult {
                    file_path,
                    file_hash: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    StickerBatchImportResult {
        success_count,
        failure_count,
        results,
    }
}

pub fn delete_sticker(app_state: &AppState, file_hash: &str) -> Result<()> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow!("failed to lock database while deleting sticker: {error}")
    })?;
    storage::db::delete_sticker(&conn, file_hash)
}

pub fn save_sticker_from_message(app_state: &AppState, file_hash: &str) -> Result<AddStickerResult> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow!("failed to lock database while saving sticker: {error}")
    })?;

    let exists_in_files: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE file_hash = ?1)",
            [file_hash],
            |row| row.get(0),
        )
        .context("Failed to check sticker file")?;

    if !exists_in_files {
        return Err(anyhow!("Sticker file is not available locally yet"));
    }

    let fallback = format!("sticker-{}.webp", &file_hash[..8.min(file_hash.len())]);
    let name: String = conn
        .query_row(
            "SELECT COALESCE(file_name, ?2) FROM files WHERE file_hash = ?1",
            rchat_storage::params![file_hash, &fallback],
            |row| row.get(0),
        )
        .unwrap_or(fallback);

    let inserted = storage::db::upsert_sticker(&conn, file_hash, Some(&name), "received")
        .context("Failed to save sticker to library")?;

    Ok(AddStickerResult {
        file_hash: file_hash.to_string(),
        name,
        converted: false,
        already_exists: !inserted,
    })
}

fn store_prepared_sticker(
    app_state: &AppState,
    prepared: PreparedSticker,
    source: &str,
) -> Result<AddStickerResult> {
    let conn = app_state.db_conn.lock().map_err(|error| {
        anyhow!("failed to lock database while adding sticker: {error}")
    })?;
    let file_hash = storage::object::create(
        &conn,
        &prepared.file_data,
        Some(&prepared.file_name),
        Some("image/webp"),
        None,
    )
    .context("Failed to store sticker file")?;

    let inserted = storage::db::upsert_sticker(&conn, &file_hash, Some(&prepared.file_name), source)
        .context("Failed to register sticker")?;

    Ok(AddStickerResult {
        file_hash,
        name: prepared.file_name,
        converted: prepared.converted,
        already_exists: !inserted,
    })
}

fn encode_webp_lossless(image: &DynamicImage) -> Result<Vec<u8>> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(&rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|error| anyhow!("Failed to encode WebP: {error}"))?;
    Ok(out)
}

fn convert_to_webp_with_auto_downscale(image: DynamicImage) -> Result<Vec<u8>> {
    let mut current = image;
    for _ in 0..=MAX_STICKER_SCALE_ATTEMPTS {
        let encoded = encode_webp_lossless(&current)?;
        if encoded.len() <= MAX_STICKER_SIZE_BYTES {
            return Ok(encoded);
        }

        let width = current.width();
        let height = current.height();
        if width <= MIN_STICKER_SIDE_PX || height <= MIN_STICKER_SIDE_PX {
            break;
        }

        let next_w = ((width as f32) * 0.85).round() as u32;
        let next_h = ((height as f32) * 0.85).round() as u32;
        let next_w = next_w.max(MIN_STICKER_SIDE_PX);
        let next_h = next_h.max(MIN_STICKER_SIDE_PX);

        if next_w == width && next_h == height {
            break;
        }
        current = current.resize(next_w, next_h, FilterType::Lanczos3);
    }

    Err(anyhow!(
        "Converted WebP sticker is still larger than 1MB after auto-compression"
    ))
}

fn sticker_name_from_path(file_path: &str) -> String {
    let stem = Path::new(file_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("sticker");
    format!("{}.webp", stem)
}

fn prepare_sticker_for_import(file_path: &str) -> Result<PreparedSticker> {
    let input_data = std::fs::read(file_path)
        .with_context(|| format!("Failed to read file '{}'", file_path))?;
    let ext = Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let file_name = sticker_name_from_path(file_path);

    match ext.as_str() {
        "webp" => {
            if input_data.len() > MAX_STICKER_SIZE_BYTES {
                return Err(anyhow!("WebP sticker exceeds 1MB limit"));
            }
            Ok(PreparedSticker {
                file_name,
                file_data: input_data,
                converted: false,
            })
        }
        "png" | "jpg" | "jpeg" => {
            let image = image::load_from_memory(&input_data)
                .map_err(|error| anyhow!("Failed to decode image: {error}"))?;
            Ok(PreparedSticker {
                file_name,
                file_data: convert_to_webp_with_auto_downscale(image)?,
                converted: true,
            })
        }
        _ => Err(anyhow!(
            "Unsupported sticker format. Use .webp directly or import .png/.jpg/.jpeg"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::profile::test_app_state;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn prepare_rejects_unsupported_format() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("sticker.txt");
        std::fs::write(&path, b"hello").expect("write");

        let error = prepare_sticker_for_import(path.to_str().expect("path"))
            .expect_err("unsupported");
        assert!(error.to_string().contains("Unsupported sticker format"));
    }

    #[tokio::test]
    async fn sticker_add_list_delete_preserves_conversion() {
        let (_temp, app_state) = test_app_state().await;
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("sticker.png");
        let image = ImageBuffer::from_pixel(12, 12, Rgba([10, 20, 30, 255]));
        DynamicImage::ImageRgba8(image).save(&path).expect("save png");

        let added = add_sticker(&app_state, path.to_str().expect("path")).expect("add sticker");
        assert!(added.converted);
        assert_eq!(added.name, "sticker.webp");

        let stickers = list_stickers(&app_state).expect("stickers");
        assert_eq!(stickers.len(), 1);
        assert_eq!(stickers[0].file_hash, added.file_hash);

        delete_sticker(&app_state, &added.file_hash).expect("delete sticker");
        assert!(list_stickers(&app_state).expect("stickers").is_empty());
    }

    #[tokio::test]
    async fn imported_sticker_uses_default_object_storage_root() {
        let (_temp, app_state) = test_app_state().await;
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("loadable-sticker.png");
        let image = ImageBuffer::from_fn(19, 17, |x, y| {
            Rgba([
                ((x * 13 + y * 5) % 255) as u8,
                ((x * 3 + y * 17) % 255) as u8,
                ((x * 23 + y * 7) % 255) as u8,
                255,
            ])
        });
        DynamicImage::ImageRgba8(image).save(&path).expect("save png");

        let added = add_sticker(&app_state, path.to_str().expect("path")).expect("add sticker");
        let conn = app_state.db_conn.lock().expect("db");

        let data = crate::storage::object::load(&conn, &added.file_hash, None)
            .expect("normal object loader can read imported sticker");

        assert!(!data.is_empty());
    }
}
