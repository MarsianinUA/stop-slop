mod args;
mod check;
mod error;
mod list;
mod output;

use std::process::ExitCode;

use clap::Parser;

use args::Command;
use error::Exit;

fn main() -> ExitCode {
    let result = match Command::parse() {
        Command::Check(args) => check::run(args),
        Command::Rules { json } => list::run(json),
    };
    let exit = result.unwrap_or_else(|error| {
        eprintln!("stop-slop: {error}");
        Exit::Failure
    });
    ExitCode::from(exit as u8)
}
