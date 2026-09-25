//! Screenshots: a frame as a PNG image.

use crate::video::Frame;
use std::path::Path;

/// `frame` as a PNG file's bytes.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&frame.rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The picture in PNG file bytes, as RGB, if it is one.
pub fn decode(bytes: &[u8]) -> Option<Frame> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    if info.color_type != png::ColorType::Rgb || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(info.buffer_size());
    Some(Frame { width: info.width, height: info.height, rgb: pixels })
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
        let mut frame = Frame::new(3, 2);
        frame.rgb = (0..18).collect();
        let bytes = encode(&frame).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&pixels[..info.buffer_size()], &frame.rgb[..]);
    }
}
