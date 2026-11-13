//! A recording of a program's frames (`toadforce::capture`) played
//! through the software renderer and the OpenGL host, to find the draws
//! the two draw differently and what those draws have in common.
//!
//! Twice. First whole, each renderer from the recording's start to its
//! end, which gives the pictures a run of the program would show. Then
//! draw by draw: before each draw the host's side is given the software
//! side's target, so that what the two make of this one draw is seen
//! without what earlier draws left different. A pixel the draw leaves
//! more than `OVER` apart is counted for the draw's state, and as one
//! the software wrote alone, the host wrote alone, or both wrote
//! differently: the first two are a difference of coverage (a
//! triangle's edge, the depth test, the alpha test, a colour key), the
//! last of the colour.
//!
//! Each draw of points or lines, and of triangles drawn as their
//! corners or edges, is printed with the pixels the software side
//! changed and the rectangle they are in, which tells what on the
//! screen the program draws that way.
//!
//! `TOADFORCE_REPLAY_SET` changes states on both sides before every
//! draw, to see which state a difference hangs on: a list such as
//! `rs26=0,ts18=0` of render states (`rs`) and of stage states of
//! every stage (`ts`) by their numbers, here the dither and the mip
//! filter off.
//! `TOADFORCE_REPLAY_OVER` is the difference above which a pixel is
//! counted, in place of `OVER`.
//! `TOADFORCE_REPLAY_PIXEL` names pixels, as `X,Y` joined by `;`, to
//! follow through the whole recording on the software side: every
//! draw that changes one is printed with the pixel before and after it
//! and the draw's steps, which tells how many layers a pixel is made
//! of and what each one adds.

use super::tests::Bench;
use crate::savestate::{Reader, State};
use crate::toadforce::ToadForce;
use crate::toadforce::capture::{Event, Recording};
use crate::toadforce::command::{self, Command};
use crate::toadforce::format::Surface;
use crate::toadforce::level1::states::STAGES;
use crate::toadforce::level1::{self, Vertices, encode};
use crate::toadforce::raster::pipeline::{self, DrawState};
use crate::toadforce::raster::texel;
use std::collections::HashMap;

/// A difference of a channel above this is one to explain: more than
/// two steps of a channel of five bits.
const OVER: u32 = 17;

/// `OVER`, or what `TOADFORCE_REPLAY_OVER` has in its place.
fn over() -> u32 {
    let set = std::env::var("TOADFORCE_REPLAY_OVER").ok();
    set.and_then(|v| v.parse().ok()).unwrap_or(OVER)
}

/// The pixels `TOADFORCE_REPLAY_PIXEL` names.
fn traced() -> Vec<(u32, u32)> {
    let Ok(list) = std::env::var("TOADFORCE_REPLAY_PIXEL") else {
        return Vec::new();
    };
    let pixels = list.split(';').filter(|item| !item.is_empty());
    pixels
        .map(|item| {
            let (x, y) = item.split_once(',').expect("X,Y");
            (x.parse().expect("an x"), y.parse().expect("a y"))
        })
        .collect()
}

/// The commands that set the states `TOADFORCE_REPLAY_SET` names.
fn overrides() -> Vec<u32> {
    let mut words = Vec::new();
    let Ok(list) = std::env::var("TOADFORCE_REPLAY_SET") else {
        return words;
    };
    for item in list.split(',').filter(|item| !item.is_empty()) {
        let (name, value) = item.split_once('=').expect("STATE=VALUE");
        let value: u32 = value.parse().expect("a value");
        let id: u32 = name[2..].parse().expect("rsN or tsN");
        match &name[..2] {
            "rs" => encode::render_state(&mut words, &[(id, value)]),
            "ts" => {
                for stage in 0..STAGES as u32 {
                    encode::stage_state(&mut words, stage, &[(id, value)]);
                }
            }
            _ => panic!("rsN or tsN"),
        }
    }
    words
}

struct Side {
    card: ToadForce,
    memory: Vec<u8>,
}

impl Side {
    fn new(rec: &Recording) -> Self {
        let mut card = ToadForce::new(rec.memory.len() as u32, 1);
        card.load(&mut Reader::new(&rec.state)).unwrap();
        Side {
            card,
            memory: rec.memory.clone(),
        }
    }

    fn submit(&mut self, words: &[u32]) {
        self.card
            .submit(words, &mut self.memory, super::tests::live());
    }

    /// Bytes written by something that isn't the card.
    fn patch(&mut self, at: usize, bytes: &[u8]) {
        let range = (at as u32, (at + bytes.len()) as u32);
        self.card.settle(&mut self.memory, Some(range));
        self.memory[at..at + bytes.len()].copy_from_slice(bytes);
        self.card.wrote(range.0, range.1 - range.0);
    }

    fn pixel(&self, s: &Surface, x: u32, y: u32) -> [u8; 4] {
        let at = s.at(x, y);
        let bytes = s.format.bytes() as usize;
        let mut word = [0; 4];
        word[..bytes].copy_from_slice(&self.memory[at..at + bytes]);
        texel::decode(s.format, u32::from_le_bytes(word))
    }
}

/// The largest difference of a colour channel. A format without alpha
/// decodes it as the same on both sides.
fn apart(a: [u8; 4], b: [u8; 4]) -> u32 {
    (0..4).map(|i| a[i].abs_diff(b[i]) as u32).max().unwrap()
}

/// What a state's draws came to.
#[derive(Default)]
struct Group {
    draws: u64,
    /// Pixels over `OVER`: written by the software alone, by the host
    /// alone, by both.
    software: u64,
    host: u64,
    both: u64,
    largest: u32,
    /// The draw with the most pixels over, and one of its pixels.
    worst: Option<Worst>,
}

struct Worst {
    over: u64,
    event: usize,
    word: usize,
    at: (u32, u32),
    software: [u8; 4],
    host: [u8; 4],
    before: [u8; 4],
    vertices: String,
    state: String,
}

/// What of a state says how it is drawn, without its numbers.
fn describe(st: &DrawState) -> String {
    let mut out = format!(
        "target {:?} depth {:?}\n  steps {:?}\n",
        st.target.colour.format,
        st.target.depth.map(|d| d.format),
        st.steps
    );
    for (stage, tex) in st.tex.iter().enumerate() {
        if let Some(t) = tex {
            out += &format!(
                "  tex{stage} {:?} {}x{} levels {} cube {} mag {:?} min {:?} mip {:?} \
                 bias {} first {} aniso {} key {:?} address {:?}\n",
                t.texture.format,
                t.texture.width,
                t.texture.height,
                t.texture.levels,
                t.texture.cube,
                t.mag,
                t.min,
                t.mip,
                t.lod_bias,
                t.first_level,
                t.anisotropy,
                t.key.is_some(),
                t.address
            );
        }
    }
    out
}

/// The numbers of the first vertices of a draw.
fn vertices(draw: &level1::Draw, memory: &[u8]) -> String {
    let n = draw.fvf.words;
    let mut out = format!(
        "primitive {} fvf {:#x} ({} words) count {}",
        draw.primitive,
        draw.fvf.code,
        n,
        draw.vertex_count()
    );
    let word = |i: usize, k: usize| match draw.vertices {
        Vertices::Inline(words) => words.get(i * n + k).copied(),
        Vertices::Buffer { offset, stride, .. } => {
            let at = offset as usize + i * stride as usize + 4 * k;
            let bytes = memory.get(at..at + 4)?;
            Some(u32::from_le_bytes(bytes.try_into().unwrap()))
        }
    };
    for i in 0..draw.vertex_count().min(6) {
        out += "\n    ";
        for k in 0..n {
            let Some(w) = word(i, k) else { break };
            let f = f32::from_bits(w);
            // A colour reads badly as a float.
            if f.abs() > 1e-6 && f.abs() < 1e7 {
                out += &format!("{f} ");
            } else {
                out += &format!("{w:#x} ");
            }
        }
    }
    out
}

fn histogram(name: &str, a: &Side, b: &Side, s: &Surface) -> Vec<bool> {
    let limit = over();
    let mut bins = [0u64; 6];
    let mut over = vec![false; (s.width * s.height) as usize];
    let (mut largest, mut sum) = (0, 0u64);
    for y in 0..s.height {
        for x in 0..s.width {
            let d = apart(a.pixel(s, x, y), b.pixel(s, x, y));
            let bin = match d {
                0 => 0,
                1 => 1,
                2 => 2,
                3..=8 => 3,
                9..=32 => 4,
                _ => 5,
            };
            bins[bin] += 1;
            sum += d as u64;
            largest = largest.max(d);
            over[(y * s.width + x) as usize] = d > limit;
        }
    }
    let n = (s.width * s.height) as f64;
    let pc = |v: u64| 100.0 * v as f64 / n;
    println!(
        "{name} {}x{} {:?} at {:#x}: 0:{:.2}% 1:{:.2}% 2:{:.2}% 3-8:{:.2}% 9-32:{:.2}% \
         33+:{:.2}% over {limit}:{:.3}% largest {largest} mean {:.3}",
        s.width,
        s.height,
        s.format,
        s.offset,
        pc(bins[0]),
        pc(bins[1]),
        pc(bins[2]),
        pc(bins[3]),
        pc(bins[4]),
        pc(bins[5]),
        pc(over.iter().filter(|&&o| o).count() as u64),
        sum as f64 / n
    );
    over
}

/// Play `rec` both ways and print what differs.
pub fn replay(bench: &Bench, rec: &Recording) {
    let limit = over();
    let set = overrides();
    // Whole, each side by itself.
    let mut software = Side::new(rec);
    let mut hosted = Side::new(rec);
    hosted.card.set_host(Some(bench.host()), &mut hosted.memory);
    let mut targets: Vec<Surface> = Vec::new();
    let traced = traced();
    let mut draw_at = 0u64;
    for event in &rec.events {
        match event {
            Event::Patch(at, bytes) => {
                software.patch(*at as usize, bytes);
                hosted.patch(*at as usize, bytes);
            }
            Event::Run(words) => {
                let mut at = 0;
                while at < words.len() {
                    let len = command::length(words[at]).max(1);
                    let one = &words[at..(at + len).min(words.len())];
                    let level = software.card.level();
                    let size = software.memory.len() as u32;
                    let draws = matches!(
                        command::decode(one, level, size),
                        Ok(Command::Level1(level1::Command::Draw(_)))
                    );
                    let mut followed = None;
                    if draws {
                        software.submit(&set);
                        hosted.submit(&set);
                        if let Ok(st) = pipeline::lower(&software.card.level1) {
                            if !targets.contains(&st.target.colour) {
                                targets.push(st.target.colour);
                            }
                            let s = st.target.colour;
                            let inside = traced.iter().filter(|p| p.0 < s.width && p.1 < s.height);
                            let was: Vec<_> = inside
                                .map(|&(x, y)| (x, y, software.pixel(&s, x, y)))
                                .collect();
                            followed = Some((st, was));
                        }
                        draw_at += 1;
                    }
                    software.submit(one);
                    hosted.submit(one);
                    if let Some((st, was)) = followed {
                        let s = st.target.colour;
                        for (x, y, was) in was {
                            let now = software.pixel(&s, x, y);
                            if now != was {
                                println!(
                                    "pixel ({x}, {y}) of {:?} at {:#x}, draw {draw_at}: {was:?} \
                                     to {now:?}\n  steps {:?}",
                                    s.format, s.offset, st.steps
                                );
                            }
                        }
                    }
                    at += len;
                }
            }
        }
    }
    hosted.card.settle(&mut hosted.memory, None);
    let status = hosted.card.describe(super::tests::live());
    println!("whole: host {}", status["host"]);
    let mut whole = HashMap::new();
    for s in &targets {
        whole.insert(s.offset, histogram("whole", &software, &hosted, s));
    }

    // Draw by draw, the host's side from the software's target.
    let mut a = Side::new(rec);
    let mut b = Side::new(rec);
    b.card.set_host(Some(bench.host()), &mut b.memory);
    let mut groups: HashMap<String, Group> = HashMap::new();
    // For each target, the state that last left each pixel over.
    let mut culprit: HashMap<u32, Vec<Option<String>>> = HashMap::new();
    let (mut draws, mut declined_before) = (0u64, 0u64);
    for (event_at, event) in rec.events.iter().enumerate() {
        let words = match event {
            Event::Patch(at, bytes) => {
                a.patch(*at as usize, bytes);
                b.patch(*at as usize, bytes);
                continue;
            }
            Event::Run(words) => words,
        };
        // The same memory at each run's start.
        b.card.settle(&mut b.memory, None);
        b.memory.copy_from_slice(&a.memory);
        b.card.wrote(0, b.memory.len() as u32);
        let mut at = 0;
        while at < words.len() {
            let len = command::length(words[at]).max(1);
            let one = &words[at..(at + len).min(words.len())];
            let word_at = at;
            at += len;
            let (level, size) = (a.card.level(), a.memory.len() as u32);
            let draw = match command::decode(one, level, size) {
                Ok(Command::Level1(level1::Command::Draw(draw))) => draw,
                _ => {
                    a.submit(one);
                    b.submit(one);
                    continue;
                }
            };
            a.submit(&set);
            b.submit(&set);
            let Ok(st) = pipeline::lower(&a.card.level1) else {
                a.submit(one);
                b.submit(one);
                continue;
            };
            draws += 1;
            let colour = st.target.colour;
            // The host's side starts from the software's buffers.
            b.card.settle(&mut b.memory, None);
            let mut ranges = vec![(colour.offset as usize, colour.end() as usize)];
            if let Some(d) = st.target.depth_surface() {
                ranges.push((d.offset as usize, d.end() as usize));
            }
            for &(first, end) in &ranges {
                let end = end.min(a.memory.len());
                if b.memory[first..end] != a.memory[first..end] {
                    b.memory[first..end].copy_from_slice(&a.memory[first..end]);
                    b.card.wrote(first as u32, (end - first) as u32);
                }
            }
            let before = a.memory[ranges[0].0..ranges[0].1.min(a.memory.len())].to_vec();
            let pixel_before = |x: u32, y: u32| {
                let at = colour.at(x, y) - colour.offset as usize;
                let bytes = colour.format.bytes() as usize;
                let mut word = [0; 4];
                word[..bytes].copy_from_slice(&before[at..at + bytes]);
                texel::decode(colour.format, u32::from_le_bytes(word))
            };
            a.submit(one);
            b.submit(one);
            b.card.settle(&mut b.memory, None);

            let key = describe(&st);
            let map = culprit
                .entry(colour.offset)
                .or_insert_with(|| vec![None; (colour.width * colour.height) as usize]);
            let group = groups.entry(key.clone()).or_default();
            group.draws += 1;
            let mut over = 0;
            // What the software side changed: how many pixels, and the
            // rectangle they are in.
            // `FILLMODE` 1 and 2 only: the rasteriser fills a triangle
            // of any other value.
            let thin = draw.primitive < 4 || matches!(st.shape.fill, 1 | 2);
            let (mut changed, mut bounds) = (0u32, [u32::MAX, u32::MAX, 0, 0]);
            // The pixel that differs most: how far, where, and the
            // software's, the host's and the earlier colour.
            type Most = (u32, u32, u32, [u8; 4], [u8; 4], [u8; 4]);
            let mut sample: Option<Most> = None;
            for y in 0..colour.height {
                for x in 0..colour.width {
                    let (pa, pb) = (a.pixel(&colour, x, y), b.pixel(&colour, x, y));
                    let was = pixel_before(x, y);
                    if pa == was && pb == was {
                        continue;
                    }
                    if pa != was {
                        changed += 1;
                        bounds = [
                            bounds[0].min(x),
                            bounds[1].min(y),
                            bounds[2].max(x),
                            bounds[3].max(y),
                        ];
                    }
                    let d = apart(pa, pb);
                    let slot = &mut map[(y * colour.width + x) as usize];
                    if d <= limit {
                        *slot = None;
                        continue;
                    }
                    *slot = Some(key.clone());
                    over += 1;
                    group.largest = group.largest.max(d);
                    if pb == was {
                        group.software += 1;
                    } else if pa == was {
                        group.host += 1;
                    } else {
                        group.both += 1;
                    }
                    if sample.is_none_or(|(sd, ..)| d > sd) {
                        sample = Some((d, x, y, pa, pb, was));
                    }
                }
            }
            if thin {
                // None for a draw that changed nothing.
                let bounds = (changed > 0).then_some(bounds);
                println!(
                    "points or lines: draw {draws}, primitive {} fill {} of {} vertices, \
                     {changed} pixels changed in {bounds:?}, {over} over; {key}",
                    draw.primitive,
                    st.shape.fill,
                    draw.vertex_count(),
                );
            }
            if let Some((_, x, y, pa, pb, was)) = sample
                && group.worst.as_ref().is_none_or(|w| over > w.over)
            {
                group.worst = Some(Worst {
                    over,
                    event: event_at,
                    word: word_at,
                    at: (x, y),
                    software: pa,
                    host: pb,
                    before: was,
                    vertices: vertices(&draw, &a.memory),
                    state: {
                        let v = &st.values;
                        format!(
                            "factor {:?} alpha_ref {} fog {:?} {} {} {} z_bias {} half_centre {} \
                             cull {} flat {} clip {:?}",
                            v.factor,
                            v.alpha_ref,
                            v.fog_colour,
                            v.fog_start,
                            v.fog_end,
                            v.fog_density,
                            v.z_bias,
                            st.shape.half_centre,
                            st.shape.cull,
                            st.shape.flat,
                            st.clip
                        )
                    },
                });
            }
            let declined = b.card.describe(super::tests::live())["host"]["declined_draws"]
                .as_u64()
                .unwrap_or(0);
            if declined != declined_before {
                declined_before = declined;
                assert_eq!(over, 0, "a declined draw differs");
            }
        }
    }

    let total: u64 = groups.values().map(|g| g.software + g.host + g.both).sum();
    println!(
        "draw by draw: {draws} draws, {declined_before} declined, {} states, \
         {total} pixels over {limit}",
        groups.len()
    );
    let mut list: Vec<(&String, &Group)> = groups.iter().collect();
    list.sort_by_key(|(_, g)| std::cmp::Reverse(g.software + g.host + g.both));
    for (n, (key, g)) in list.iter().enumerate() {
        let all = g.software + g.host + g.both;
        if all == 0 {
            break;
        }
        println!(
            "state {n}: {} draws, {all} pixels over ({:.1}%): software alone {}, \
             host alone {}, both {}; largest {}\n  {key}",
            g.draws,
            100.0 * all as f64 / total.max(1) as f64,
            g.software,
            g.host,
            g.both,
            g.largest
        );
        if let Some(w) = &g.worst {
            println!(
                "  worst draw: event {} word {}, {} over; at {:?} software {:?} host {:?} \
                 before {:?}\n  {}\n  {}",
                w.event, w.word, w.over, w.at, w.software, w.host, w.before, w.state, w.vertices
            );
        }
    }
    // What of the whole pictures' differences the draws account for.
    for s in &targets {
        let (Some(over), Some(map)) = (whole.get(&s.offset), culprit.get(&s.offset)) else {
            continue;
        };
        let all = over.iter().filter(|&&o| o).count();
        let mut by: HashMap<&String, usize> = HashMap::new();
        let mut none = 0;
        for (o, c) in over.iter().zip(map) {
            if !o {
                continue;
            }
            match c {
                Some(key) => *by.entry(key).or_default() += 1,
                None => none += 1,
            }
        }
        if all == 0 {
            continue;
        }
        println!(
            "target {:#x}: {all} pixels over in the whole picture, {none} with no draw to \
             name",
            s.offset
        );
        let mut by: Vec<(&String, usize)> = by.into_iter().collect();
        by.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
        for (key, count) in by {
            let n = list.iter().position(|(k, _)| *k == key).unwrap();
            println!(
                "  state {n}: {count} ({:.1}%)",
                100.0 * count as f64 / all as f64
            );
        }
    }
}
