//! A game's manuals and extras: PDF documents and pictures (PNG, JPEG,
//! GIF) of its manual, code wheel, maps and reference cards, which the
//! settings window shows over the game (config_ui/manual.rs), for the
//! copy protection's questions and the rest.
//!
//! A game profile lists them with `[game]`'s `manual=` lines, and the
//! files in its extras folder, `games/<id>.extras`, are its too
//! (`games::manuals`). JPEG pictures and PDF documents need the `manuals`
//! feature; PDF pages are rasterized with hayro.

use std::path::{Path, PathBuf};

use crate::hostfs;
use crate::video::Frame;

/// The longest side a page is rendered at, in pixels.
const MAX_SIDE: f32 = 4096.0;

/// The extensions of the files that can be manuals.
const EXTENSIONS: &[&str] = &["pdf", "png", "jpg", "jpeg", "gif"];

/// Whether `path` is a document or picture's, by its extension.
pub fn is_manual_name(path: &Path) -> bool {
    path.extension().is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A manual or extra of a game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manual {
    pub path: PathBuf,
    /// What the list shows it as: its own title, or its file's name.
    pub title: String,
}

impl Manual {
    /// `manual=`'s value: a path, then `|` and a title if it has one.
    /// Relative paths are from `dir`.
    pub fn parse(value: &str, dir: &Path, home: Option<&Path>) -> Manual {
        let (path, title) = match value.split_once('|') {
            Some((path, title)) => (path.trim(), Some(title.trim())),
            None => (value.trim(), None),
        };
        let path = crate::mount::expand_host_path(path, dir, home);
        let title = title.filter(|t| !t.is_empty()).map_or_else(|| title_of(&path), str::to_string);
        Manual { path, title }
    }
}

/// A file's name without its extension.
pub fn title_of(path: &Path) -> String {
    path.file_stem().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

enum Kind {
    Picture(Frame),
    #[cfg(feature = "manuals")]
    Pdf(hayro::hayro_syntax::Pdf),
}

/// A document or picture, opened.
pub struct Document {
    kind: Kind,
}

impl Document {
    pub fn open(path: &Path) -> Result<Document, String> {
        let error = |e: String| format!("{}: {}", path.display(), e);
        let data = hostfs::read(path).map_err(|e| error(e.to_string()))?;
        let extension = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let kind = match extension.as_str() {
            "png" => Kind::Picture(decode_png(&data).map_err(error)?),
            "gif" => Kind::Picture(decode_gif(&data).map_err(error)?),
            #[cfg(feature = "manuals")]
            "jpg" | "jpeg" => Kind::Picture(decode_jpeg(&data).map_err(error)?),
            #[cfg(feature = "manuals")]
            "pdf" => {
                let pdf = hayro::hayro_syntax::Pdf::new(data).map_err(|e| error(format!("{:?}", e)))?;
                if pdf.pages().is_empty() {
                    return Err(error("the document has no pages".to_string()));
                }
                Kind::Pdf(pdf)
            }
            _ => return Err(error("this build shows PNG and GIF pictures only".to_string())),
        };
        Ok(Document { kind })
    }

    pub fn pages(&self) -> usize {
        match &self.kind {
            Kind::Picture(_) => 1,
            #[cfg(feature = "manuals")]
            Kind::Pdf(pdf) => pdf.pages().len(),
        }
    }

    /// A page's size: a picture's in pixels, a PDF page's in points.
    #[cfg_attr(not(feature = "manuals"), allow(unused_variables))]
    pub fn page_size(&self, page: usize) -> (f32, f32) {
        match &self.kind {
            Kind::Picture(picture) => (picture.width as f32, picture.height as f32),
            #[cfg(feature = "manuals")]
            Kind::Pdf(pdf) => pdf.pages().get(page).map_or((612.0, 792.0), |p| p.render_dimensions()),
        }
    }

    /// The page `width` x `height` pixels big (at most `MAX_SIDE` on its
    /// longest side, and then scaled up by the viewer).
    #[cfg_attr(not(feature = "manuals"), allow(unused_variables))]
    pub fn render(&self, page: usize, width: u32, height: u32) -> Frame {
        let fit = (MAX_SIDE / width.max(height) as f32).min(1.0);
        let (width, height) = (((width as f32 * fit) as u32).max(1), ((height as f32 * fit) as u32).max(1));
        match &self.kind {
            Kind::Picture(picture) => scale(picture, width, height),
            #[cfg(feature = "manuals")]
            Kind::Pdf(pdf) => {
                use hayro::vello_cpu::color::palette::css::WHITE;
                let Some(p) = pdf.pages().get(page) else { return Frame::new(width, height) };
                let (w, h) = p.render_dimensions();
                let settings = hayro::RenderSettings {
                    x_scale: width as f32 / w,
                    y_scale: height as f32 / h,
                    width: Some(width as u16),
                    height: Some(height as u16),
                    bg_color: WHITE,
                };
                let cache = hayro::RenderCache::new();
                let pixmap = hayro::render(p, &cache, &Default::default(), &settings);
                let mut frame = Frame::new(pixmap.width() as u32, pixmap.height() as u32);
                // On white: opaque, so premultiplied is as it is.
                for (rgb, rgba) in frame.rgb.chunks_exact_mut(3).zip(pixmap.data_as_u8_slice().chunks_exact(4)) {
                    rgb.copy_from_slice(&rgba[..3]);
                }
                frame
            }
        }
    }
}

/// Pixels with an alpha channel, on white.
fn on_white(width: u32, height: u32, rgba: &[u8]) -> Frame {
    let mut frame = Frame::new(width, height);
    for (rgb, p) in frame.rgb.chunks_exact_mut(3).zip(rgba.chunks_exact(4)) {
        let a = p[3] as u32;
        for i in 0..3 {
            rgb[i] = ((p[i] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    frame
}

fn decode_png(data: &[u8]) -> Result<Frame, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("the picture is too big")?];
    let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    let (width, height) = (info.width, info.height);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => pixels[..info.buffer_size()].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels[..info.buffer_size()].chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => pixels[..info.buffer_size()].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("an indexed picture that wasn't expanded".to_string()),
    };
    Ok(on_white(width, height, &rgba))
}

/// A GIF's first picture.
fn decode_gif(data: &[u8]) -> Result<Frame, String> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let (width, height) = (decoder.width() as u32, decoder.height() as u32);
    let mut canvas = vec![255u8; (width * height * 4) as usize];
    let frame = decoder.read_next_frame().map_err(|e| e.to_string())?.ok_or("the GIF has no pictures")?;
    for y in 0..frame.height as u32 {
        for x in 0..frame.width as u32 {
            let (cx, cy) = (x + frame.left as u32, y + frame.top as u32);
            if cx < width && cy < height {
                let from = ((y * frame.width as u32 + x) * 4) as usize;
                let to = ((cy * width + cx) * 4) as usize;
                canvas[to..to + 4].copy_from_slice(&frame.buffer[from..from + 4]);
            }
        }
    }
    Ok(on_white(width, height, &canvas))
}

#[cfg(feature = "manuals")]
fn decode_jpeg(data: &[u8]) -> Result<Frame, String> {
    use jpeg_decoder::PixelFormat;
    let mut decoder = jpeg_decoder::Decoder::new(data);
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no picture")?;
    let mut frame = Frame::new(info.width as u32, info.height as u32);
    match info.pixel_format {
        PixelFormat::RGB24 => {
            let len = frame.rgb.len();
            frame.rgb.copy_from_slice(&pixels[..len]);
        }
        PixelFormat::L8 => {
            for (rgb, &g) in frame.rgb.chunks_exact_mut(3).zip(&pixels) {
                rgb.fill(g);
            }
        }
        PixelFormat::L16 => {
            for (rgb, g) in frame.rgb.chunks_exact_mut(3).zip(pixels.chunks_exact(2)) {
                rgb.fill(g[0]);
            }
        }
        PixelFormat::CMYK32 => {
            for (rgb, p) in frame.rgb.chunks_exact_mut(3).zip(pixels.chunks_exact(4)) {
                let k = 255 - p[3] as u32;
                for i in 0..3 {
                    rgb[i] = ((255 - p[i] as u32) * k / 255) as u8;
                }
            }
        }
    }
    Ok(frame)
}

/// `picture` scaled to `width` x `height`: each pixel the average of the
/// pixels it covers, or the nearest one when it is scaled up.
pub fn scale(picture: &Frame, width: u32, height: u32) -> Frame {
    let mut out = Frame::new(width, height);
    let (sw, sh) = (picture.width as usize, picture.height as usize);
    if sw == 0 || sh == 0 {
        return out;
    }
    // The source pixels from `i` to before the next.
    let span = |i: usize, step: f32, len: usize| {
        let start = ((i as f32 * step) as usize).min(len - 1);
        (start, (((i + 1) as f32 * step) as usize).clamp(start + 1, len))
    };
    let (fx, fy) = (sw as f32 / width as f32, sh as f32 / height as f32);
    for y in 0..height as usize {
        let (y0, y1) = span(y, fy, sh);
        for x in 0..width as usize {
            let (x0, x1) = span(x, fx, sw);
            let mut sum = [0u32; 3];
            for sy in y0..y1 {
                for at in (sy * sw + x0..sy * sw + x1).map(|p| p * 3) {
                    for (s, &c) in sum.iter_mut().zip(&picture.rgb[at..at + 3]) {
                        *s += c as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let at = (y * width as usize + x) * 3;
            for (o, s) in out.rgb[at..at + 3].iter_mut().zip(sum) {
                *o = (s / n) as u8;
            }
        }
    }
    out
}

/// The part `x, y, width, height` of `picture`, clipped to it.
pub fn crop(picture: &Frame, x: u32, y: u32, width: u32, height: u32) -> Frame {
    let x = x.min(picture.width);
    let y = y.min(picture.height);
    let width = width.min(picture.width - x);
    let height = height.min(picture.height - y);
    let mut out = Frame::new(width, height);
    let row = width as usize * 3;
    for r in 0..height as usize {
        let from = ((y as usize + r) * picture.width as usize + x as usize) * 3;
        out.rgb[r * row..(r + 1) * row].copy_from_slice(&picture.rgb[from..from + row]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, data: &[u8]) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-manuals");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    /// A 2x2 PNG: red, green, blue and transparent.
    fn png() -> Vec<u8> {
        let mut data = Vec::new();
        let mut encoder = png::Encoder::new(&mut data, 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0]).unwrap();
        writer.finish().unwrap();
        data
    }

    #[test]
    fn pictures_are_one_page_on_white() {
        let doc = Document::open(&scratch("card.png", &png())).unwrap();
        assert_eq!((doc.pages(), doc.page_size(0)), (1, (2.0, 2.0)));
        let page = doc.render(0, 2, 2);
        assert_eq!(page.rgb, [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        let half = doc.render(0, 1, 1);
        assert_eq!(half.rgb, [127, 127, 127]);
        let big = doc.render(0, 4, 4);
        assert_eq!(&big.rgb[..6], [255, 0, 0, 255, 0, 0]);
    }

    #[test]
    fn a_gif_is_its_first_picture() {
        let mut data = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut data, 2, 1, &[0, 0, 0, 255, 255, 255]).unwrap();
            let mut frame = gif::Frame::default();
            (frame.width, frame.height, frame.buffer) = (2, 1, std::borrow::Cow::Borrowed(&[0u8, 1][..]));
            encoder.write_frame(&frame).unwrap();
        }
        let doc = Document::open(&scratch("map.gif", &data)).unwrap();
        assert_eq!(doc.render(0, 2, 1).rgb, [0, 0, 0, 255, 255, 255]);
    }

    #[cfg(feature = "manuals")]
    #[test]
    fn a_pdf_page_is_rendered() {
        // One page, 200 by 100 points, with a black square at its left.
        let content = "0 0 0 rg 0 0 100 100 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, object).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            pdf.extend(format!("{:010} 00000 n \n", offset).bytes());
        }
        pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", objects.len() + 1, xref).bytes());
        let doc = Document::open(&scratch("manual.pdf", &pdf)).unwrap();
        assert_eq!((doc.pages(), doc.page_size(0)), (1, (200.0, 100.0)));
        let page = doc.render(0, 40, 20);
        assert_eq!((page.width, page.height), (40, 20));
        let pixel = |x: usize, y: usize| &page.rgb[(y * 40 + x) * 3..][..3];
        assert_eq!(pixel(5, 10), [0, 0, 0], "the square");
        assert_eq!(pixel(35, 10), [255, 255, 255], "the paper");
    }

    #[test]
    fn manual_lines_have_a_path_and_a_title() {
        let dir = Path::new("/games");
        assert_eq!(
            Manual::parse("keen/manual.pdf | Manual", dir, None),
            Manual { path: "/games/keen/manual.pdf".into(), title: "Manual".into() }
        );
        assert_eq!(Manual::parse("/x/Code Wheel.png", dir, None).title, "Code Wheel");
        assert!(is_manual_name(Path::new("a.JPG")) && !is_manual_name(Path::new("a.txt")));
    }
}
