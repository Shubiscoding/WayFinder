// image_conv.rs — Pure-Rust image conversion using the `image` crate.

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::path::Path;

/// Composite transparency onto white (JPEG has no alpha channel).
fn flatten_on_white(img: &DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

pub fn convert_image(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Err(format!(
            "Source file \"{}\" does not exist. Check that the file was not moved or deleted.",
            source.display()
        ));
    }

    let img = image::open(source).map_err(|e| {
        format!(
            "Could not read image \"{}\": {e}. Check that the file is a valid image and not corrupted.",
            source.display()
        )
    })?;

    let ext = destination
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let format = match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" | "jfif" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        "bmp" => ImageFormat::Bmp,
        "tiff" | "tif" => ImageFormat::Tiff,
        _ => {
            return Err(format!(
                "Unsupported destination image format \".{ext}\". Supported formats: png, jpg, webp, bmp, tiff."
            ));
        }
    };

    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!("Could not create folder \"{}\": {e}. Check folder permissions.", parent.display())
            })?;
        }
    }

    // Hand each encoder a colour type it actually supports.
    let prepared = match format {
        ImageFormat::Jpeg => DynamicImage::ImageRgb8(flatten_on_white(&img)),
        ImageFormat::WebP | ImageFormat::Bmp => {
            if img.color().has_alpha() {
                DynamicImage::ImageRgba8(img.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(img.to_rgb8())
            }
        }
        _ => img,
    };

    prepared.save_with_format(destination, format).map_err(|e| {
        format!(
            "Could not write converted image to \"{}\": {e}. Check disk space and permissions.",
            destination.display()
        )
    })
}
