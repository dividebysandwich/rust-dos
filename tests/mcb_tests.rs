use rust_dos::bus::Bus;
use rust_dos::mcb::{self, END_OF_CONVENTIONAL, FIRST_MCB_SEG, MCB_Z, walk};
use std::path::PathBuf;

fn fresh_bus() -> Bus {
    Bus::new(PathBuf::from("."))
}

#[test]
fn init_empty_single_free_block() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let chain = walk(&bus);
    assert_eq!(chain.len(), 1);
    let (seg, m) = chain[0];
    assert_eq!(seg, FIRST_MCB_SEG);
    assert_eq!(m.signature, MCB_Z);
    assert!(m.is_free());
    // All of conventional memory minus the header paragraph.
    assert_eq!(m.size, END_OF_CONVENTIONAL - FIRST_MCB_SEG - 1);
}

#[test]
fn alloc_split_updates_chain() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let seg = mcb::alloc(&mut bus, 0x1234, 0x100).expect("alloc should succeed");
    assert_eq!(seg, FIRST_MCB_SEG + 1); // First usable paragraph

    let chain = walk(&bus);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].1.owner, 0x1234);
    assert_eq!(chain[0].1.size, 0x100);
    assert!(chain[1].1.is_free());
    assert_eq!(chain[1].1.signature, MCB_Z);
}

#[test]
fn alloc_too_large_returns_max_free() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let avail = END_OF_CONVENTIONAL - FIRST_MCB_SEG - 1;
    let err = mcb::alloc(&mut bus, 0x1234, avail + 10).expect_err("should fail");
    // The reported max is the largest free block currently available.
    assert_eq!(err, avail);
}

#[test]
fn free_then_coalesce() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let a = mcb::alloc(&mut bus, 0x1000, 0x100).unwrap();
    let b = mcb::alloc(&mut bus, 0x1000, 0x100).unwrap();
    let c = mcb::alloc(&mut bus, 0x1000, 0x100).unwrap();

    // Free middle and then outer blocks; coalescing should produce a single
    // free trailing block again.
    mcb::free(&mut bus, b).unwrap();
    mcb::free(&mut bus, a).unwrap();
    mcb::free(&mut bus, c).unwrap();

    let chain = walk(&bus);
    assert_eq!(chain.len(), 1, "chain should collapse back to single free block, got {:?}", chain);
    assert!(chain[0].1.is_free());
    assert_eq!(chain[0].1.signature, MCB_Z);
}

#[test]
fn resize_shrink_creates_free_block() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let seg = mcb::alloc(&mut bus, 0x1000, 0x400).unwrap();
    let chain_before = walk(&bus);
    assert_eq!(chain_before.len(), 2);

    mcb::resize(&mut bus, seg, 0x100).unwrap();
    let chain = walk(&bus);
    // We split a free block out of the shrink, which then coalesces with the
    // tail free block => still 2 blocks.
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].1.size, 0x100);
    assert!(chain[1].1.is_free());
}

#[test]
fn resize_grow_into_adjacent_free() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let seg = mcb::alloc(&mut bus, 0x1000, 0x10).unwrap();

    // Growing beyond the block should succeed while the tail is free.
    mcb::resize(&mut bus, seg, 0x200).unwrap();
    let chain = walk(&bus);
    assert_eq!(chain[0].1.size, 0x200);
}

#[test]
fn resize_grow_bounded_by_tail() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let full = END_OF_CONVENTIONAL - FIRST_MCB_SEG - 1;
    let seg = mcb::alloc(&mut bus, 0x1000, 0x10).unwrap();
    // The block can grow to its current size + 1 (absorbed free header) +
    // the entire remaining free block.
    let err = mcb::resize(&mut bus, seg, full + 1).expect_err("too big");
    assert_eq!(err, full);
}

#[test]
fn free_owned_by_releases_chain_blocks() {
    let mut bus = fresh_bus();
    mcb::init_empty(&mut bus);
    let _a = mcb::alloc(&mut bus, 0x1234, 0x100).unwrap();
    let _b = mcb::alloc(&mut bus, 0x5678, 0x100).unwrap();
    let _c = mcb::alloc(&mut bus, 0x1234, 0x100).unwrap();

    mcb::free_owned_by(&mut bus, 0x1234);
    let chain = walk(&bus);
    // Owner 0x5678 still has its block; everything else coalesced into frees.
    let owned_by_1234 = chain.iter().filter(|(_, m)| m.owner == 0x1234).count();
    assert_eq!(owned_by_1234, 0);
    let owned_by_5678 = chain.iter().filter(|(_, m)| m.owner == 0x5678).count();
    assert_eq!(owned_by_5678, 1);
}
