//! Screenshots: a frame as a PNG image.

use crate::video::Frame;
use std::path::Path;

/// `frame` as a PNG file's bytes.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&frame.rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The picture in PNG file bytes. Old RGB slot pictures still load.
pub fn decode(bytes: &[u8]) -> Option<Frame> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(info.buffer_size());
    match info.color_type {
        png::ColorType::Rgb => Some(Frame::from_rgb(info.width, info.height, pixels)),
        png::ColorType::Rgba => Some(Frame::from_rgba(info.width, info.height, pixels)),
        _ => None,
    }
}

/// Save `frame` as a PNG file at `path`.
pub fn save(frame: &Frame, path: &Path) -> Result<(), String> {
    std::fs::write(path, encode(frame)?).map_err(|e| format!("{}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_decodes_back() {
        let frame = Frame::from_rgb(3, 2, (0..18).collect());
        let bytes = encode(&frame).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&pixels[..info.buffer_size()], &frame.rgba[..]);
        assert_eq!(decode(&encode(&frame).unwrap()), Some(frame));
    }

    #[test]
    fn old_rgb_slot_pictures_still_load_as_opaque_rgba() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3, 4, 5, 6]).unwrap();
        }
        assert_eq!(decode(&bytes).unwrap().rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
