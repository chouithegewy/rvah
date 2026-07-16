//! `rvah` — developer CLI and Phase 1 mock UI (FR-027): validate, package,
//! sign, install, and exercise add-ons against the simulator runtime.

mod commands;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(commands::run(&args));
}
