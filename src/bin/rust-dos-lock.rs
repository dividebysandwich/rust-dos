//! The machine-wide run lock's wrapper and status tool, on its own: holds
//! the lock shared or exclusive around any command, and lists who holds
//! it (crates/runlock has the protocol). It needs neither SDL nor a sound
//! library, so one copy installed outside every checkout serves the whole
//! machine, builds into a target directory included.

fn main() {
    let args = std::env::args_os().skip(1).collect();
    std::process::exit(rust_dos_runlock::cli::main("rust-dos-lock", args));
}
