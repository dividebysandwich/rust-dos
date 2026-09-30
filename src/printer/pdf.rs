//! PDF documents of printed pages: each page one picture, compressed with
//! zlib, on a sheet of the page's size. The pages go into the file as
//! they come, and `finish` writes the page tree and the cross-reference
//! table after them, so a long print job never has to be in memory at
//! once.

use super::page::Page;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

/// The catalog and the page tree have the first two objects; the page
/// tree is written last, when all its pages are known.
const CATALOG: usize = 1;
const PAGES: usize = 2;

pub struct PdfWriter {
    path: PathBuf,
    out: BufWriter<File>,
    /// Where each object starts in the file (objects 1 and 2 at the end).
    offsets: Vec<u64>,
    pages: Vec<usize>,
}

impl PdfWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut out = BufWriter::new(file);
        // The binary comment marks the file as binary for transfer tools.
        out.write_all(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n").map_err(|e| e.to_string())?;
        Ok(Self { path: path.to_path_buf(), out, offsets: vec![0; 3], pages: Vec::new() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Start object `n`, which the next `offsets` entry is for.
    fn begin(&mut self, n: usize) -> std::io::Result<()> {
        let at = self.out.stream_position()?;
        if n >= self.offsets.len() {
            self.offsets.resize(n + 1, 0);
        }
        self.offsets[n] = at;
        writeln!(self.out, "{} 0 obj", n)
    }

    fn next_object(&self) -> usize {
        self.offsets.len()
    }

    /// Add `page` to the document.
    pub fn add_page(&mut self, page: &Page) -> Result<(), String> {
        self.write_page(page).map_err(|e| format!("{}: {}", self.path.display(), e))
    }

    fn write_page(&mut self, page: &Page) -> std::io::Result<()> {
        let gray = page.is_gray();
        let pixels = if gray { page.to_gray() } else { page.to_rgb() };
        let mut z = ZlibEncoder::new(Vec::new(), Compression::new(6));
        z.write_all(&pixels)?;
        let data = z.finish()?;
        let (w_in, h_in) = page.inches();
        let (w_pt, h_pt) = (w_in * 72.0, h_in * 72.0);

        let image = self.next_object();
        self.begin(image)?;
        write!(
            self.out,
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /{} /BitsPerComponent 8 \
             /Filter /FlateDecode /Length {} >>\nstream\n",
            page.width,
            page.height,
            if gray { "DeviceGray" } else { "DeviceRGB" },
            data.len()
        )?;
        self.out.write_all(&data)?;
        self.out.write_all(b"\nendstream\nendobj\n")?;

        let content = format!("q {:.3} 0 0 {:.3} 0 0 cm /Im0 Do Q\n", w_pt, h_pt);
        let contents = self.next_object();
        self.begin(contents)?;
        write!(self.out, "<< /Length {} >>\nstream\n{}endstream\nendobj\n", content.len(), content)?;

        let page_object = self.next_object();
        self.begin(page_object)?;
        write!(
            self.out,
            "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 {:.3} {:.3}] /Contents {} 0 R \
             /Resources << /XObject << /Im0 {} 0 R >> >> >>\nendobj\n",
            PAGES, w_pt, h_pt, contents, image
        )?;
        self.pages.push(page_object);
        Ok(())
    }

    /// Write the page tree, the catalog and the cross-reference table, and
    /// close the file. Returns its path.
    pub fn finish(mut self) -> Result<PathBuf, String> {
        self.write_end().map_err(|e| format!("{}: {}", self.path.display(), e))?;
        Ok(self.path)
    }

    fn write_end(&mut self) -> std::io::Result<()> {
        self.begin(PAGES)?;
        let kids: Vec<String> = self.pages.iter().map(|n| format!("{} 0 R", n)).collect();
        write!(self.out, "<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n", kids.join(" "), self.pages.len())?;
        self.begin(CATALOG)?;
        write!(self.out, "<< /Type /Catalog /Pages {} 0 R >>\nendobj\n", PAGES)?;
        let xref = self.out.stream_position()?;
        write!(self.out, "xref\n0 {}\n0000000000 65535 f \n", self.offsets.len())?;
        for offset in &self.offsets[1..] {
            writeln!(self.out, "{:010} 00000 n ", offset)?;
        }
        write!(
            self.out,
            "trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{}\n%%EOF\n",
            self.offsets.len(),
            CATALOG,
            xref
        )?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::page::{BLACK, CYAN, FULL};

    #[test]
    fn a_document_has_its_pages_and_a_table_of_them() {
        let dir = std::env::temp_dir().join(format!("rust-dos-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.pdf");
        let mut pdf = PdfWriter::create(&path).unwrap();
        let mut page = Page::new(8.5, 11.0, 36);
        page.ink(10, 10, FULL, BLACK);
        pdf.add_page(&page).unwrap();
        page.ink(20, 10, FULL, CYAN);
        pdf.add_page(&page).unwrap();
        assert_eq!(pdf.finish().unwrap(), path);

        let bytes = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/MediaBox [0 0 612.000 792.000]"));
        assert!(text.contains("/DeviceGray") && text.contains("/DeviceRGB"));
        // Each object is where the table says it is.
        let xref_at: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        let table = &text[xref_at..];
        for (n, line) in table.lines().skip(3).take_while(|l| l.ends_with(" n ")).enumerate() {
            let offset: usize = line[..10].parse().unwrap();
            assert!(text[offset..].starts_with(&format!("{} 0 obj", n + 1)), "object {}", n + 1);
        }
        // A reader, if the host has one, takes it.
        if let Ok(out) = std::process::Command::new("pdfinfo").arg(&path).output() {
            let info = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            assert!(info.contains("Pages:") && info.contains("2"), "{}", info);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
