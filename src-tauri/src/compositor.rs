use crate::error::AppError;
use image::{DynamicImage, GenericImageView, RgbaImage};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Resize an image to completely fill the target dimensions, cropping excess from center.
///
/// The image is scaled so that the smaller dimension matches the target, then the
/// overflowing dimension is center-cropped.
fn crop_to_fill(img: &DynamicImage, target_w: u32, target_h: u32) -> RgbaImage {
    let (src_w, src_h) = img.dimensions();

    // Calculate scale factor: we need to cover the entire target area,
    // so scale by the larger ratio.
    let scale_x = target_w as f64 / src_w as f64;
    let scale_y = target_h as f64 / src_h as f64;
    let scale = scale_x.max(scale_y);

    let scaled_w = (src_w as f64 * scale).round() as u32;
    let scaled_h = (src_h as f64 * scale).round() as u32;

    // Resize to the scaled dimensions (covers target fully)
    let resized = img.resize_exact(scaled_w, scaled_h, image::imageops::FilterType::Lanczos3);

    // Center-crop to exact target dimensions
    let crop_x = (scaled_w.saturating_sub(target_w)) / 2;
    let crop_y = (scaled_h.saturating_sub(target_h)) / 2;

    resized.crop_imm(crop_x, crop_y, target_w, target_h).to_rgba8()
}

/// Composite multiple images onto a single canvas matching the bounding box of all monitors.
///
/// Each image is resized to fill its corresponding monitor's area using `crop_to_fill`.
/// `image_paths[i]` maps to `monitors[i]`. If there are fewer images than monitors,
/// the last image is reused for remaining monitors. The result is saved as PNG.
pub fn composite_wallpaper(
    image_paths: &[PathBuf],
    monitors: &[MonitorGeometry],
    output_path: &Path,
) -> Result<(), AppError> {
    if monitors.is_empty() {
        return Err(AppError::Wallpaper("No monitors provided".to_string()));
    }
    if image_paths.is_empty() {
        return Err(AppError::Wallpaper("No images provided".to_string()));
    }

    // Calculate the bounding box of all monitors.
    // Monitors can have negative offsets, so we need to find the true min/max.
    let min_x = monitors.iter().map(|m| m.x).min().unwrap();
    let min_y = monitors.iter().map(|m| m.y).min().unwrap();
    let max_x = monitors
        .iter()
        .map(|m| m.x + m.width as i32)
        .max()
        .unwrap();
    let max_y = monitors
        .iter()
        .map(|m| m.y + m.height as i32)
        .max()
        .unwrap();

    let canvas_w = (max_x - min_x) as u32;
    let canvas_h = (max_y - min_y) as u32;

    let mut canvas = RgbaImage::new(canvas_w, canvas_h);

    // Load images
    let images: Vec<DynamicImage> = image_paths
        .iter()
        .map(|p| {
            image::open(p).map_err(|e| AppError::Wallpaper(format!("Failed to open {}: {}", p.display(), e)))
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (i, monitor) in monitors.iter().enumerate() {
        // Use the corresponding image, or the last one if fewer images than monitors
        let img = &images[i.min(images.len() - 1)];

        let filled = crop_to_fill(img, monitor.width, monitor.height);

        // Calculate where this monitor sits on the canvas (offset by bounding box origin)
        let canvas_x = (monitor.x - min_x) as u32;
        let canvas_y = (monitor.y - min_y) as u32;

        // Copy the filled image onto the canvas
        image::imageops::overlay(
            &mut canvas,
            &filled,
            canvas_x as i64,
            canvas_y as i64,
        );
    }

    canvas
        .save(output_path)
        .map_err(|e| AppError::Wallpaper(format!("Failed to save composite: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Create a solid-color test PNG in the temp directory and return its path.
    fn make_test_image(width: u32, height: u32, r: u8, g: u8, b: u8) -> PathBuf {
        let dir = env::temp_dir().join("stashpaper_test");
        std::fs::create_dir_all(&dir).unwrap();

        let filename = format!("test_{}x{}_{}_{}_{}_{}.png", width, height, r, g, b, rand::random::<u32>());
        let path = dir.join(filename);

        let img = RgbaImage::from_fn(width, height, |_x, _y| {
            image::Rgba([r, g, b, 255])
        });
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn test_single_monitor_resizes_to_fit() {
        let img_path = make_test_image(800, 600, 255, 0, 0);
        let output = env::temp_dir().join(format!("stashpaper_test_single_{}.png", rand::random::<u32>()));

        let monitors = vec![MonitorGeometry {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];

        composite_wallpaper(&[img_path.clone()], &monitors, &output).unwrap();

        let result = image::open(&output).unwrap();
        let (w, h) = result.dimensions();
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);

        // Clean up
        let _ = std::fs::remove_file(&img_path);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn test_two_monitors_side_by_side() {
        let red_img = make_test_image(1920, 1080, 255, 0, 0);
        let blue_img = make_test_image(1920, 1080, 0, 0, 255);
        let output = env::temp_dir().join(format!("stashpaper_test_dual_{}.png", rand::random::<u32>()));

        let monitors = vec![
            MonitorGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorGeometry {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
        ];

        composite_wallpaper(&[red_img.clone(), blue_img.clone()], &monitors, &output).unwrap();

        let result = image::open(&output).unwrap().to_rgba8();
        let (w, h) = result.dimensions();
        assert_eq!(w, 3840, "Canvas width should be 3840");
        assert_eq!(h, 1080, "Canvas height should be 1080");

        // Check left side is red (sample pixel at 100, 100)
        let left_pixel = result.get_pixel(100, 100);
        assert_eq!(left_pixel[0], 255, "Left monitor R channel should be 255");
        assert_eq!(left_pixel[1], 0, "Left monitor G channel should be 0");
        assert_eq!(left_pixel[2], 0, "Left monitor B channel should be 0");

        // Check right side is blue (sample pixel at 2000, 100)
        let right_pixel = result.get_pixel(2000, 100);
        assert_eq!(right_pixel[0], 0, "Right monitor R channel should be 0");
        assert_eq!(right_pixel[1], 0, "Right monitor G channel should be 0");
        assert_eq!(right_pixel[2], 255, "Right monitor B channel should be 255");

        // Clean up
        let _ = std::fs::remove_file(&red_img);
        let _ = std::fs::remove_file(&blue_img);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn test_monitors_with_offset() {
        let red_img = make_test_image(1920, 1080, 255, 0, 0);
        let green_img = make_test_image(2560, 1440, 0, 255, 0);
        let output = env::temp_dir().join(format!("stashpaper_test_offset_{}.png", rand::random::<u32>()));

        let monitors = vec![
            MonitorGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorGeometry {
                x: 1920,
                y: -200,
                width: 2560,
                height: 1440,
            },
        ];

        composite_wallpaper(&[red_img.clone(), green_img.clone()], &monitors, &output).unwrap();

        let result = image::open(&output).unwrap();
        let (w, h) = result.dimensions();

        // min_x=0, max_x=1920+2560=4480, min_y=-200, max_y=max(1080,1240)=1240
        // canvas_w = 4480-0 = 4480
        // canvas_h = 1240-(-200) = 1440
        assert_eq!(w, 4480, "Canvas width should be 4480");
        assert_eq!(h, 1440, "Canvas height should be 1440");

        // Clean up
        let _ = std::fs::remove_file(&red_img);
        let _ = std::fs::remove_file(&green_img);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn test_empty_monitors_returns_error() {
        let img_path = make_test_image(100, 100, 128, 128, 128);
        let output = env::temp_dir().join("stashpaper_test_empty_monitors.png");

        let result = composite_wallpaper(&[img_path.clone()], &[], &output);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("No monitors"),
            "Error should mention no monitors: {}",
            err_msg
        );

        // Clean up
        let _ = std::fs::remove_file(&img_path);
    }

    #[test]
    fn test_empty_images_returns_error() {
        let output = env::temp_dir().join("stashpaper_test_empty_images.png");

        let monitors = vec![MonitorGeometry {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];

        let result = composite_wallpaper(&[], &monitors, &output);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("No images"),
            "Error should mention no images: {}",
            err_msg
        );
    }
}
