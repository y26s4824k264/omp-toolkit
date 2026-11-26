use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, metadata::Orientation};
use rmcp::{
    model::ContentBlock,
    schemars::{self, JsonSchema},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 48_000_000;
// A 1.5 MiB image encodes to at most 2 MiB of Base64 in the MCP payload.
const MAX_PREVIEW_BYTES: usize = 1536 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    /// Absolute directory containing the source image.
    pub(crate) root: PathBuf,
    /// Relative PNG, JPEG, WebP or GIF file.
    pub(crate) file: PathBuf,
    /// Maximum preview edge in pixels, 1 to 1600, without upscaling.
    /// If omitted with no crop, return image metadata only.
    max_edge: Option<u32>,
    /// Optional rectangle in displayed image pixels, after EXIF orientation.
    crop: Option<Crop>,
}

impl Selection {
    pub(crate) fn wants_preview(&self) -> bool {
        self.max_edge.is_some() || self.crop.is_some()
    }
}

#[derive(Clone, Copy, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
struct Crop {
    /// Zero-based left edge.
    x: u32,
    /// Zero-based top edge.
    y: u32,
    width: u32,
    height: u32,
}

pub(crate) fn read(path: &Path, selection: Selection) -> Result<Vec<ContentBlock>> {
    let size = path.metadata()?.len();
    ensure!(
        size <= MAX_FILE_BYTES,
        "image exceeds the 64 MiB input limit"
    );
    let max_edge = selection.max_edge.unwrap_or(768);
    ensure!(
        (1..=1600).contains(&max_edge),
        "max_edge must be between 1 and 1600"
    );
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format().context("unrecognized image format")?;
    ensure!(
        matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
        ),
        "use a PNG, JPEG, WebP or GIF image"
    );
    let mut limits = Limits::default();
    limits.max_alloc = Some(384 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().context("open image decoder")?;
    let (mut width, mut height) = decoder.dimensions();
    let orientation = decoder.orientation().context("read image orientation")?;
    if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        std::mem::swap(&mut width, &mut height);
    }
    let mut info = json!({"width": width, "height": height, "bytes": size, "mime_type": format.to_mime_type()});
    if !selection.wants_preview() {
        return Ok(vec![ContentBlock::text(info.to_string())]);
    }

    ensure!(
        u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "image preview source exceeds 48 megapixels"
    );
    if let Some(crop) = selection.crop {
        ensure!(
            crop.width > 0
                && crop.height > 0
                && crop
                    .x
                    .checked_add(crop.width)
                    .is_some_and(|right| right <= width)
                && crop
                    .y
                    .checked_add(crop.height)
                    .is_some_and(|bottom| bottom <= height),
            "crop lies outside the {width}x{height} image"
        );
    }
    let mut image = DynamicImage::from_decoder(decoder).context("decode preview source")?;
    image.apply_orientation(orientation);
    if let Some(crop) = selection.crop {
        image = image.crop_imm(crop.x, crop.y, crop.width, crop.height);
        info["crop"] = json!(crop);
    }
    if image.width().max(image.height()) > max_edge {
        image = image.resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3);
    }
    let mut bytes = Cursor::new(Vec::new());
    let mut preview_format = if format == ImageFormat::Jpeg {
        ImageFormat::Jpeg
    } else {
        ImageFormat::Png
    };
    image.write_to(&mut bytes, preview_format)?;
    if bytes.get_ref().len() > MAX_PREVIEW_BYTES {
        // Lossless WebP preserves transparency and sharp linework.
        preview_format = ImageFormat::WebP;
        bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, preview_format)?;
    }
    ensure!(
        bytes.get_ref().len() <= MAX_PREVIEW_BYTES,
        "preview exceeds 1.5 MiB; select a smaller crop or max_edge"
    );
    let mime_type = preview_format.to_mime_type();
    info["preview"] =
        json!({"width": image.width(), "height": image.height(), "mime_type": mime_type});
    Ok(vec![
        ContentBlock::text(info.to_string()),
        ContentBlock::image(STANDARD.encode(bytes.into_inner()), mime_type),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_has_no_pixels_and_preview_crops_before_downscaling() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.png");
        let mut image = image::RgbaImage::from_pixel(80, 40, image::Rgba([255, 0, 0, 255]));
        for x in 30..70 {
            for y in 10..30 {
                image.put_pixel(x, y, image::Rgba([0, 180, 0, 128]));
            }
        }
        image.save(&path).unwrap();
        let mut args = json!({"root":directory.path(),"file":"source.png"});
        let metadata = read(&path, serde_json::from_value(args.clone()).unwrap()).unwrap();
        assert_eq!(metadata.len(), 1);
        assert!(metadata[0].as_text().is_some());
        args["max_edge"] = json!(20);
        args["crop"] = json!({"x":30,"y":10,"width":40,"height":20});
        let cropped = read(&path, serde_json::from_value(args.clone()).unwrap()).unwrap();
        let preview = cropped[1].as_image().unwrap();
        let pixels = image::load_from_memory(&STANDARD.decode(&preview.data).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(pixels.dimensions(), (20, 10));
        assert_eq!(pixels.get_pixel(10, 5), &image::Rgba([0, 180, 0, 128]));
        args["crop"]["x"] = json!(u32::MAX);
        let error = read(&path, serde_json::from_value(args).unwrap()).unwrap_err();
        assert!(error.to_string().contains("crop lies outside"));
    }
}
