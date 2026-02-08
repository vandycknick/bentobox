mod commands;

use crate::commands::BentoCtlCmd;
use std::process::ExitCode;

use clap::Parser;
use eyre::Report;

fn main() -> ExitCode {
    let cmd = BentoCtlCmd::parse();

    match cmd.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            print_error(&err, cmd.verbose);
            ExitCode::FAILURE
        }
    }
}

fn print_error(err: &Report, verbose: u8) {
    eprintln!("\x1b[31merror:\x1b[0m {}", err);

    if verbose == 0 {
        if err.chain().nth(1).is_some() {
            eprintln!("hint: run with -v to see the full error chain");
        }
        return;
    }

    let mut last = err.to_string();
    let mut idx = 0usize;
    for cause in err.chain().skip(1) {
        let msg = cause.to_string();
        if msg == last {
            continue;
        }
        idx += 1;
        eprintln!("  {}. {}", idx, msg);
        last = msg;
    }
}
