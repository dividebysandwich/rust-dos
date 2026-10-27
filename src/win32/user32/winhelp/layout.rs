//! Laying a help topic's paragraphs out in a window of a given width:
//! words wrapped into lines, each piece of text placed with its font,
//! and the places of the links, which the viewer draws and hit-tests.

use super::hlpfile::{Align, Item, Link, Paragraph};
use crate::win32::gdi32::Rect;

/// What laying text out needs to know of the fonts: the width of text in
/// one, and its height and ascent.
pub trait Measure {
    fn width(&mut self, font: u16, text: &[u8]) -> i32;
    fn height(&mut self, font: u16) -> (i32, i32);
}

/// A piece of text, placed: its top left corner, font and link.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub ascent: i32,
    pub text: Vec<u8>,
    pub font: u16,
    pub link: Option<usize>,
}

/// A picture, placed.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub number: u16,
    pub link: Option<usize>,
}

/// A topic, laid out.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub runs: Vec<Run>,
    pub pictures: Vec<Placed>,
    pub links: Vec<Link>,
    /// The height of it all, and the right edge of the widest line.
    pub height: i32,
    pub width: i32,
}

impl Layout {
    /// The link at (`x`, `y`), if there is one there.
    pub fn link_at(&self, x: i32, y: i32) -> Option<usize> {
        let runs = self
            .runs
            .iter()
            .map(|r| (Rect::sized(r.x, r.y, r.width, r.height), r.link));
        let pictures = self
            .pictures
            .iter()
            .map(|p| (Rect::sized(p.x, p.y, p.width, p.height), p.link));
        runs.chain(pictures)
            .find(|(r, l)| l.is_some() && r.contains(x, y))
            .and_then(|(_, l)| l)
    }
}

/// Half points in pixels, at 96 pixels an inch.
pub fn pixels(half_points: i32) -> i32 {
    half_points * 2 / 3
}

/// The distance between the default tab stops: half an inch.
const TAB: i32 = 48;

/// A piece of a word: text in one font, in one link or none.
#[derive(Clone, Debug)]
struct Seg {
    text: Vec<u8>,
    font: u16,
    link: Option<usize>,
}

enum Tok {
    /// Text that the line can't break in: it can break after it.
    Word(Vec<Seg>),
    Tab,
    Break,
    Picture(u16, Option<usize>),
}

/// A thing on the line being filled.
enum Piece {
    Text(Seg, i32),
    Picture(u16, Option<usize>, i32, i32),
}

/// The line being filled: its pieces and where each starts.
struct Line {
    pieces: Vec<(i32, Piece)>,
    x: i32,
}

/// The parts of a paragraph as words and the like. `font` is the font
/// in use, which a font change in it changes for what follows.
fn tokens(par: &Paragraph, font: &mut u16, links: &mut Vec<Link>) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut word: Vec<Seg> = Vec::new();
    let mut link = None;
    let close = |word: &mut Vec<Seg>, out: &mut Vec<Tok>| {
        if !word.is_empty() {
            out.push(Tok::Word(std::mem::take(word)));
        }
    };
    for item in &par.items {
        match item {
            Item::Text(text) => {
                let mut start = 0;
                for i in 0..text.len() {
                    let end_of_spaces = text[i] == b' ' && text.get(i + 1) != Some(&b' ');
                    if end_of_spaces {
                        word.push(Seg {
                            text: text[start..=i].to_vec(),
                            font: *font,
                            link,
                        });
                        close(&mut word, &mut out);
                        start = i + 1;
                    }
                }
                if start < text.len() {
                    word.push(Seg {
                        text: text[start..].to_vec(),
                        font: *font,
                        link,
                    });
                }
            }
            Item::Font(f) => *font = *f,
            Item::LineBreak => {
                close(&mut word, &mut out);
                out.push(Tok::Break);
            }
            Item::Tab => {
                close(&mut word, &mut out);
                out.push(Tok::Tab);
            }
            Item::Link(l) => {
                links.push(l.clone());
                link = Some(links.len() - 1);
            }
            Item::LinkEnd => link = None,
            Item::Picture(n) => {
                close(&mut word, &mut out);
                out.push(Tok::Picture(*n, link));
            }
        }
    }
    close(&mut word, &mut out);
    out
}

/// Lay the paragraphs `pars` out `width` pixels wide, with `margin`
/// pixels left free on each side and at the top. `picture` gives a
/// picture's size.
pub fn layout(
    pars: &[Paragraph],
    width: i32,
    margin: i32,
    m: &mut dyn Measure,
    picture: &mut dyn FnMut(u16) -> Option<(i32, i32)>,
) -> Layout {
    let mut out = Layout::default();
    let mut font = 0u16;
    let mut y = margin;
    for par in pars {
        let left = margin + pixels(par.left);
        let right = (width - margin - pixels(par.right)).max(left + 1);
        let first = (left + pixels(par.first)).max(margin);
        y += pixels(par.space_before);
        let toks = tokens(par, &mut font, &mut out.links);
        let mut line = Line {
            pieces: Vec::new(),
            x: first,
        };
        // Where the paragraph's first line goes.
        let top = y;
        macro_rules! flush {
            () => {{
                place(&line, &mut y, &mut out, m, par, left, right, font);
                line.pieces.clear();
                line.x = left;
            }};
        }
        for tok in toks {
            match tok {
                Tok::Word(segs) => {
                    let widths: Vec<i32> = segs.iter().map(|s| m.width(s.font, &s.text)).collect();
                    let total: i32 = widths.iter().sum();
                    let last = segs.last().map_or(0, |s| {
                        let trimmed = s.text.len() - trailing_spaces(&s.text);
                        m.width(s.font, &s.text[..trimmed])
                    });
                    let ink = total - widths.last().copied().unwrap_or(0) + last;
                    if line.x + ink > right && !line.pieces.is_empty() {
                        flush!();
                    }
                    for (seg, w) in segs.into_iter().zip(widths) {
                        let x = line.x;
                        line.pieces.push((x, Piece::Text(seg, w)));
                        line.x += w;
                    }
                }
                Tok::Tab => {
                    line.x = next_tab(par, line.x, margin, left);
                }
                Tok::Break => flush!(),
                Tok::Picture(n, link) => {
                    let (w, h) = picture(n).unwrap_or((0, 0));
                    if line.x + w > right && !line.pieces.is_empty() {
                        flush!();
                    }
                    let x = line.x;
                    line.pieces.push((x, Piece::Picture(n, link, w, h)));
                    line.x += w;
                }
            }
        }
        if !line.pieces.is_empty() || y == top {
            flush!();
        }
        y += pixels(par.space_after);
    }
    out.height = y + margin;
    out
}

/// The number of spaces at the end of `text`.
pub fn trailing_spaces(text: &[u8]) -> usize {
    text.iter().rev().take_while(|&&c| c == b' ').count()
}

/// Where a tab at `x` goes on to: the next of the paragraph's tab stops,
/// the left indent of a paragraph with a hanging first line, or the next
/// default stop. Stops count from `margin`.
fn next_tab(par: &Paragraph, x: i32, margin: i32, left: i32) -> i32 {
    let mut stops: Vec<i32> = par.tabs.iter().map(|&(p, _)| margin + pixels(p)).collect();
    if left > margin {
        stops.push(left);
    }
    stops.sort();
    if let Some(&s) = stops.iter().find(|&&s| s > x) {
        return s;
    }
    let from = x - margin;
    margin + (from / TAB + 1) * TAB
}

/// Put the line's pieces in `out` at `y`, lined up on one baseline and
/// as the paragraph aligns them, and move `y` below the line.
#[allow(clippy::too_many_arguments)]
fn place(
    line: &Line,
    y: &mut i32,
    out: &mut Layout,
    m: &mut dyn Measure,
    par: &Paragraph,
    left: i32,
    right: i32,
    font: u16,
) {
    let (mut height, mut ascent) = m.height(font);
    if !line.pieces.is_empty() {
        height = 0;
        ascent = 0;
        let mut descent = 0;
        for (_, p) in &line.pieces {
            let (h, a) = match p {
                Piece::Text(s, _) => m.height(s.font),
                Piece::Picture(_, _, _, h) => (*h, *h),
            };
            ascent = ascent.max(a);
            descent = descent.max(h - a);
        }
        height = height.max(ascent + descent);
    }
    if par.line_spacing > 0 {
        height = height.max(pixels(par.line_spacing));
    }
    // The line's ink ends before the spaces at its end.
    let end = line.pieces.last().map_or(left, |(x, p)| match p {
        Piece::Text(s, w) => {
            let n = s.text.len() - trailing_spaces(&s.text);
            x + if n == s.text.len() {
                *w
            } else {
                m.width(s.font, &s.text[..n])
            }
        }
        Piece::Picture(_, _, w, _) => x + w,
    });
    let start = line.pieces.first().map_or(left, |(x, _)| *x);
    let shift = match par.align {
        Align::Left => 0,
        Align::Right => (right - end).max(0),
        Align::Center => ((right - end) - (start - left)).max(0) / 2,
    };
    for (x, p) in &line.pieces {
        match p {
            Piece::Text(s, w) => {
                let (h, a) = m.height(s.font);
                out.runs.push(Run {
                    x: x + shift,
                    y: *y + ascent - a,
                    width: *w,
                    height: h,
                    ascent: a,
                    text: s.text.clone(),
                    font: s.font,
                    link: s.link,
                });
            }
            Piece::Picture(n, link, w, h) => out.pictures.push(Placed {
                x: x + shift,
                y: *y + ascent - h,
                width: *w,
                height: *h,
                number: *n,
                link: *link,
            }),
        }
    }
    out.width = out.width.max(end + shift);
    *y += height;
}

#[cfg(test)]
mod tests {
    use super::super::hlpfile::Target;
    use super::*;

    /// Every character 6 pixels wide; fonts 10 high with an ascent of 8.
    struct Fixed;

    impl Measure for Fixed {
        fn width(&mut self, _font: u16, text: &[u8]) -> i32 {
            text.len() as i32 * 6
        }
        fn height(&mut self, _font: u16) -> (i32, i32) {
            (10, 8)
        }
    }

    fn par(items: Vec<Item>) -> Paragraph {
        Paragraph {
            items,
            ..Paragraph::default()
        }
    }

    fn text(s: &str) -> Item {
        Item::Text(s.as_bytes().to_vec())
    }

    #[test]
    fn words_wrap_at_the_right_margin() {
        // 10 characters fit in 60 pixels between margins of 0.
        let pars = [par(vec![text("aaa bbb ccc ddd")])];
        let l = layout(&pars, 60, 0, &mut Fixed, &mut |_| None);
        let lines: Vec<(i32, i32, &[u8])> = l
            .runs
            .iter()
            .map(|r| (r.x, r.y, r.text.as_slice()))
            .collect();
        assert_eq!(
            lines,
            [
                (0, 0, &b"aaa "[..]),
                (24, 0, b"bbb "),
                (0, 10, b"ccc "),
                (24, 10, b"ddd"),
            ]
        );
        assert_eq!(l.height, 20);
    }

    #[test]
    fn a_hanging_indent_tabs_to_the_left_indent() {
        // Indented 144 half points (96 pixels), the first line 96 less.
        let mut p = par(vec![text("Key:"), Item::Tab, text("does it")]);
        p.left = 144;
        p.first = -144;
        let l = layout(&[p], 400, 0, &mut Fixed, &mut |_| None);
        assert_eq!(l.runs[0].x, 0);
        assert_eq!(l.runs[1].x, 96);
    }

    #[test]
    fn links_are_found_where_their_text_is() {
        let link = Link {
            target: Target::Jump {
                file: None,
                hash: 17,
            },
            marked: true,
        };
        let pars = [par(vec![
            text("see "),
            Item::Link(link.clone()),
            text("this"),
            Item::LinkEnd,
            text(" topic"),
        ])];
        let l = layout(&pars, 400, 4, &mut Fixed, &mut |_| None);
        assert_eq!(l.links, [link]);
        // "see " is 24 pixels from the margin of 4.
        assert_eq!(l.link_at(4 + 24 + 1, 5), Some(0));
        assert_eq!(l.link_at(4 + 1, 5), None);
    }

    #[test]
    fn an_empty_paragraph_is_a_blank_line() {
        let pars = [par(vec![text("a")]), par(vec![]), par(vec![text("b")])];
        let l = layout(&pars, 400, 0, &mut Fixed, &mut |_| None);
        assert_eq!(l.runs[1].y, 20);
    }

    #[test]
    fn centred_lines_are_centred() {
        let mut p = par(vec![text("abcd")]);
        p.align = Align::Center;
        let l = layout(&[p], 100, 0, &mut Fixed, &mut |_| None);
        assert_eq!(l.runs[0].x, (100 - 24) / 2);
    }
}
