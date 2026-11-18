use eframe::egui;
use std::io::Cursor;

pub(super) fn decode(
    bytes: &[u8],
    display_dimension: u32,
) -> Result<egui::ColorImage, Box<dyn std::error::Error>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode()?;
    let pixels = if decoded.width().max(decoded.height()) > display_dimension {
        decoded
            .thumbnail(display_dimension, display_dimension)
            .into_rgba8()
    } else {
        decoded.into_rgba8()
    };
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [pixels.width() as usize, pixels.height() as usize],
        pixels.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larger_source_images_are_scaled_to_thumbnail_size() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(600, 450)
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        assert_eq!(decode(bytes.get_ref(), 512).unwrap().size, [512, 384]);
    }
}
