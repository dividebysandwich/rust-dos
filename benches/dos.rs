//! The built-in DOS's file calls on a folder mounted from the host: what
//! DIR, a game's file scan and a game's data file loads cost.
//!
//! The folders are generated at start-up under the target directory. 218
//! entries is the size of Dark Sun 2's game folder.

mod common;

use common::*;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

/// Folder sizes: a small game, Dark Sun 2's folder, a large one.
const SIZES: [usize; 3] = [50, 218, 1000];

/// DIR: one listing of the whole folder (what the shell's DIR asks for).
fn dir_listing(c: &mut Criterion) {
    let mut group = c.benchmark_group("dos/dir_listing");
    for count in SIZES {
        let root = game_folder(&format!("dir_listing_{count}"), count);
        let cpu = machine(&root);
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| black_box(cpu.bus.disk.list_directory(r"C:\*.*", 0x16).unwrap().len()))
        });
    }
    group.finish();
}

/// FindFirst then FindNext until the end, through INT 21h, as a program's
/// own directory scan or a DIR-like tool does. Few samples and no 1000:
/// one run over 218 entries takes over a second on main.
fn find_first_next(c: &mut Criterion) {
    let mut group = c.benchmark_group("dos/find_first_next");
    group.sample_size(10);
    for count in [50, 218] {
        let root = game_folder(&format!("find_first_next_{count}"), count);
        let mut cpu = machine(&root);
        set_dta(&mut cpu);
        assert_eq!(find_all(&mut cpu, r"C:\*.*", 0x16), count);
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| black_box(find_all(&mut cpu, r"C:\*.*", 0x16)))
        });
    }
    group.finish();
}

/// A game looking for one kind of file in its folder: "RGN*.GFF" over
/// Dark Sun 2's 218 entries.
fn find_pattern(c: &mut Criterion) {
    let root = game_folder("find_pattern", 218);
    let mut cpu = machine(&root);
    set_dta(&mut cpu);
    assert!(find_all(&mut cpu, r"C:\RGN*.GFF", 0) > 0);
    c.bench_function("dos/find_pattern/218", |b| {
        b.iter(|| black_box(find_all(&mut cpu, r"C:\RGN*.GFF", 0)))
    });
}

/// Opening and closing a data file three folders deep, each folder holding
/// a game folder's worth of entries: C:\GAMES\DARKSUN\DATA\RGN0D2.GFF.
fn open_deep(c: &mut Criterion) {
    let root = scratch("open_deep");
    let games = root.join("GAMES");
    write_files(&games, &game_names(50));
    let game = games.join("DARKSUN");
    write_files(&game, &game_names(218));
    let data = game.join("DATA");
    write_files(&data, &game_names(218));
    let mut cpu = machine(&root);
    let path = r"C:\GAMES\DARKSUN\DATA\RGN0D2.GFF";
    open_close(&mut cpu, path);
    c.bench_function("dos/open_close_deep", |b| b.iter(|| open_close(&mut cpu, path)));
}

criterion_group!(benches, dir_listing, find_first_next, find_pattern, open_deep);
criterion_main!(benches);
