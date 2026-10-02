//! The game regression suite: real DOS games, downloaded on first use,
//! played by scripts on machines set up in many ways. Opt-in, as it
//! downloads games and takes a while:
//!
//!   cargo test --release --test game_suite -- --ignored --nocapture
//!
//! `GAME_SUITE=doom,keen` picks scenarios, `GAME_SUITE_BLESS=1` records
//! the hashes their pictures and sounds should have. See
//! docs/game-suite.md.

mod gamesuite;

/// Download and install every game.
#[test]
#[ignore]
fn fetch() {
    let mut failed = Vec::new();
    for game in gamesuite::catalog::games() {
        match gamesuite::install(&game) {
            Ok(dir) => eprintln!("[game-suite] {}: {}", game.id, dir.display()),
            Err(why) => failed.push(why),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Play the scenarios.
#[test]
#[ignore]
fn suite() {
    gamesuite::run_all(gamesuite::selected(gamesuite::catalog::scenarios()));
}
