use image::DynamicImage;
use ratatui_image::{
    Resize, ResizeEncodeRender,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};
use std::collections::HashMap;

pub(crate) const ART_RADIUS_NORMAL: u32 = 6;
pub(crate) const ART_RADIUS_FULL: u32 = 2;

pub(crate) fn with_rounded_corners(img: DynamicImage, radius_pct: u32) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let r = ((w.min(h) * radius_pct / 100) as f64).max(4.0);
    for y in 0..h {
        for x in 0..w {
            let corner = match (
                x < r as u32,
                x >= w.saturating_sub(r as u32),
                y < r as u32,
                y >= h.saturating_sub(r as u32),
            ) {
                (true, _, true, _) => Some((r as u32, r as u32)),
                (_, true, true, _) => Some((w - r as u32, r as u32)),
                (true, _, _, true) => Some((r as u32, h - r as u32)),
                (_, true, _, true) => Some((w - r as u32, h - r as u32)),
                _ => None,
            };
            if let Some((cx, cy)) = corner {
                let dx = x as f64 - cx as f64;
                let dy = y as f64 - cy as f64;
                if dx * dx + dy * dy > r * r {
                    rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                }
            }
        }
    }
    DynamicImage::ImageRgba8(rgba)
}

pub fn apply_image_protocol(picker: &mut Picker, protocol: &str) {
    match protocol {
        "halfblocks" => picker.set_protocol_type(ProtocolType::Halfblocks),
        "sixel" => picker.set_protocol_type(ProtocolType::Sixel),
        "kitty" => picker.set_protocol_type(ProtocolType::Kitty),
        "iterm2" => picker.set_protocol_type(ProtocolType::Iterm2),
        _ => {
            // "auto" or unknown: on Windows, terminal graphics protocols aren't supported
            if cfg!(target_os = "windows") {
                picker.set_protocol_type(ProtocolType::Halfblocks);
            }
        }
    }
}

/// Pre-encodes a thumbnail protocol for the fixed `THUMB_W × 2` cell thumbnail rect so
/// draw-time `needs_resize` returns `None` and rendering never blocks on encoding.
pub fn encode_thumb_protocol(picker: &Picker, img: DynamicImage) -> StatefulProtocol {
    let mut proto = picker.new_resize_protocol(img);
    let area = ratatui::layout::Size {
        width: crate::ui::THUMB_W,
        height: 2,
    };
    if let Some(sz) = proto.needs_resize(&Resize::Fit(None), area) {
        proto.resize_encode(&Resize::Fit(None), sz);
    }
    proto
}

pub fn create_album_art_protocols(
    img: &DynamicImage,
    picker: &mut Picker,
) -> (Option<StatefulProtocol>, Option<StatefulProtocol>) {
    (
        Some(picker.new_resize_protocol(with_rounded_corners(img.clone(), ART_RADIUS_NORMAL))),
        Some(picker.new_resize_protocol(with_rounded_corners(img.clone(), ART_RADIUS_FULL))),
    )
}

/// Recreate the normal/full album-art protocols from the cached image (if any), forcing the
/// terminal to retransmit at current dimensions. No-op when no artwork is cached.
pub fn refresh_album_art(
    last_artwork_image: &Option<DynamicImage>,
    picker: &mut Picker,
    album_art: &mut Option<StatefulProtocol>,
    album_art_full: &mut Option<StatefulProtocol>,
) {
    if let Some(img) = last_artwork_image {
        (*album_art, *album_art_full) = create_album_art_protocols(img, picker);
    }
}

pub fn rebuild_all_thumbnails(
    thumbnail_images: &HashMap<String, DynamicImage>,
    picker: &mut Picker,
) -> HashMap<String, StatefulProtocol> {
    thumbnail_images
        .iter()
        .map(|(url, img)| (url.clone(), picker.new_resize_protocol(img.clone())))
        .collect()
}
