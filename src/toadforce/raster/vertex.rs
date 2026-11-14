//! Vertices: the layout a draw command names with a Direct3D FVF code,
//! and a vertex as the rasteriser takes it.

use super::texel::{self, Rgba};

pub const FVF_XYZ: u32 = 0x002;
pub const FVF_XYZRHW: u32 = 0x004;
pub const FVF_NORMAL: u32 = 0x010;
pub const FVF_PSIZE: u32 = 0x020;
pub const FVF_DIFFUSE: u32 = 0x040;
pub const FVF_SPECULAR: u32 = 0x080;

/// The coordinate sets a vertex can have.
pub const SETS: usize = 8;

/// The fields of a vertex and where each is, in words from its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fvf {
    pub code: u32,
    /// The position has a reciprocal w: the vertex is in screen space.
    pub transformed: bool,
    /// Blend weights after the position, 0 to 3.
    pub weights: u8,
    pub normal: Option<u8>,
    pub psize: Option<u8>,
    pub diffuse: Option<u8>,
    pub specular: Option<u8>,
    /// For each coordinate set, where it is and how many numbers it
    /// has (0 for a set the vertex lacks).
    pub sets: [(u8, u8); SETS],
    /// Words of a vertex.
    pub words: usize,
}

impl Fvf {
    /// The layout `code` names, or None for a code that names none the
    /// card takes.
    pub fn parse(code: u32) -> Option<Self> {
        // Bit 0 and bits 12 to 15 name nothing of level 1.
        if code & 0xF001 != 0 {
            return None;
        }
        let (transformed, weights) = match code & 0x00E {
            FVF_XYZ => (false, 0),
            FVF_XYZRHW => (true, 0),
            0x006 => (false, 1),
            0x008 => (false, 2),
            0x00A => (false, 3),
            _ => return None,
        };
        // A vertex in screen space has no normal.
        if transformed && code & FVF_NORMAL != 0 {
            return None;
        }
        let mut at = 3 + transformed as u8 + weights;
        let mut field = |present: bool, words: u8| {
            present.then(|| {
                at += words;
                at - words
            })
        };
        let normal = field(code & FVF_NORMAL != 0, 3);
        let psize = field(code & FVF_PSIZE != 0, 1);
        let diffuse = field(code & FVF_DIFFUSE != 0, 1);
        let specular = field(code & FVF_SPECULAR != 0, 1);
        let count = (code >> 8 & 0xF) as usize;
        if count > SETS {
            return None;
        }
        let mut sets = [(0, 0); SETS];
        for (set, slot) in sets.iter_mut().enumerate() {
            let size = code >> (16 + 2 * set) & 3;
            if set >= count {
                // The size of a set the vertex lacks is 0.
                if size != 0 {
                    return None;
                }
                continue;
            }
            let numbers = [2, 3, 4, 1][size as usize];
            *slot = (field(true, numbers).unwrap(), numbers);
        }
        Some(Fvf {
            code,
            transformed,
            weights,
            normal,
            psize,
            diffuse,
            specular,
            sets,
            words: at as usize,
        })
    }
}

/// A vertex in screen space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenVertex {
    /// In pixels of the render target.
    pub x: f32,
    pub y: f32,
    /// Depth, 0 to 1.
    pub z: f32,
    /// 1 / w.
    pub rhw: f32,
    pub diffuse: Rgba,
    pub specular: Rgba,
    /// The point's size in pixels, where the vertex has one.
    pub psize: Option<f32>,
    /// Each coordinate set as (s, t, r, q).
    pub tex: [[f32; 4]; SETS],
}

impl Default for ScreenVertex {
    fn default() -> Self {
        ScreenVertex {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            rhw: 1.0,
            diffuse: [255; 4],
            specular: [0; 4],
            psize: None,
            tex: [[0.0, 0.0, 0.0, 1.0]; SETS],
        }
    }
}

impl ScreenVertex {
    /// The vertex whose words `word` gives, laid out as `fvf`, which is
    /// a transformed layout. A vertex without a diffuse colour is
    /// white, one without a specular colour has none, and a coordinate
    /// a set lacks is 0, but the fourth, which is the set's last number
    /// (`geometry::last_as_fourth`), or 1 for a set the vertex hasn't
    /// got. A reciprocal w that isn't a positive number is 1.
    pub fn read(fvf: &Fvf, word: impl Fn(usize) -> u32) -> Self {
        let number = |at: usize| f32::from_bits(word(at));
        let rhw = number(3);
        let mut v = ScreenVertex {
            x: number(0),
            y: number(1),
            z: number(2),
            rhw: if rhw > 0.0 && rhw.is_finite() {
                rhw
            } else {
                1.0
            },
            ..ScreenVertex::default()
        };
        if let Some(at) = fvf.psize {
            v.psize = Some(number(at as usize));
        }
        if let Some(at) = fvf.diffuse {
            v.diffuse = texel::from_word(word(at as usize));
        }
        if let Some(at) = fvf.specular {
            v.specular = texel::from_word(word(at as usize));
        }
        for (set, &(at, numbers)) in fvf.sets.iter().enumerate() {
            for (i, n) in v.tex[set][..numbers as usize].iter_mut().enumerate() {
                *n = number(at as usize + i);
            }
            v.tex[set] = super::geometry::last_as_fourth(v.tex[set], numbers as usize);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_lays_its_fields_out_in_direct3ds_order() {
        // D3DFVF_TLVERTEX: position with rhw, two colours, one set.
        let tl = Fvf::parse(0x1C4).unwrap();
        assert!(tl.transformed);
        assert_eq!((tl.diffuse, tl.specular), (Some(4), Some(5)));
        assert_eq!(tl.sets[0], (6, 2));
        assert_eq!(tl.sets[1], (0, 0));
        assert_eq!(tl.words, 8);
        // D3DFVF_VERTEX: position, normal, one set.
        let v = Fvf::parse(0x112).unwrap();
        assert!(!v.transformed);
        assert_eq!(v.normal, Some(3));
        assert_eq!(v.sets[0], (6, 2));
        assert_eq!(v.words, 8);
        // Two weights, a point size, and sets of 3, 1 and 4 numbers.
        let code = 0x008 | FVF_PSIZE | FVF_DIFFUSE | 0x300 | 1 << 16 | 3 << 18 | 2 << 20;
        let f = Fvf::parse(code).unwrap();
        assert_eq!(f.weights, 2);
        assert_eq!((f.psize, f.diffuse), (Some(5), Some(6)));
        assert_eq!(&f.sets[..3], [(7, 3), (10, 1), (11, 4)]);
        assert_eq!(f.words, 15);
        // The bare position.
        assert_eq!(Fvf::parse(FVF_XYZRHW).unwrap().words, 4);
        assert_eq!(Fvf::parse(0x804).unwrap().words, 20);
    }

    #[test]
    fn codes_the_card_doesnt_take_are_no_layout() {
        for code in [
            0,               // no position
            0x001,           // the reserved bit
            0x00C,           // four weights
            0x00E,           // five weights
            0x1002,          // the last weight as indices
            0x902,           // nine sets
            0x014,           // a normal in screen space
            0x104 | 1 << 18, // a size for a set that isn't there
        ] {
            assert_eq!(Fvf::parse(code), None, "{code:#X}");
        }
    }

    #[test]
    fn a_vertex_is_read_with_defaults_for_what_it_lacks() {
        let fvf = Fvf::parse(FVF_XYZRHW | FVF_DIFFUSE | 0x200 | 3 << 16 | 1 << 18).unwrap();
        let words: Vec<u32> = [
            10.5f32.to_bits(),
            20.25f32.to_bits(),
            0.5f32.to_bits(),
            0.25f32.to_bits(),
            0x8040_2010,
            7.0f32.to_bits(),
            1.0f32.to_bits(),
            2.0f32.to_bits(),
            3.0f32.to_bits(),
        ]
        .to_vec();
        assert_eq!(fvf.words, words.len());
        let v = ScreenVertex::read(&fvf, |i| words[i]);
        assert_eq!((v.x, v.y, v.z, v.rhw), (10.5, 20.25, 0.5, 0.25));
        assert_eq!(texel::to_word(v.diffuse), 0x8040_2010);
        assert_eq!(v.specular, [0; 4]);
        assert_eq!(v.psize, None);
        // A set's last number is its fourth too.
        assert_eq!(v.tex[0], [7.0, 0.0, 0.0, 7.0]);
        assert_eq!(v.tex[1], [1.0, 2.0, 3.0, 3.0]);
        assert_eq!(v.tex[2], [0.0, 0.0, 0.0, 1.0]);
        // A reciprocal w of 0, below 0 or no number is 1.
        for rhw in [0.0f32, -2.0, f32::NAN, f32::INFINITY] {
            let mut words = words.clone();
            words[3] = rhw.to_bits();
            assert_eq!(ScreenVertex::read(&fvf, |i| words[i]).rhw, 1.0);
        }
    }
}
