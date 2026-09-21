// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The `clyean` command line: launches the User Assistant in its sandbox, scaffolds
//! projects, manages the sandbox, and inspects agents, plans, and work.

mod cli;
mod commands;
mod runtime;

use clap::Parser;

fn main() {
    let arguments = cli::Cli::parse();
    runtime::init_tracing(arguments.verbose);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let exit_code = runtime.block_on(commands::run(arguments));
    std::process::exit(exit_code);
}
