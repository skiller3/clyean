// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

mod inspect;
mod launch;
mod sandbox;
mod scaffold;

use crate::cli::{Cli, Command};

pub async fn run(cli: Cli) -> i32 {
    let result = match cli.command {
        None => launch::run(&cli.project, cli.launch).await,
        Some(Command::Scaffold(args)) => scaffold::run(&cli.project, args).await,
        Some(Command::Sandbox(args)) => sandbox::run(&cli.project, args).await,
        Some(Command::Agents) => inspect::agents(&cli.project),
        Some(Command::Plans) => inspect::plans(&cli.project),
        Some(Command::Work) => inspect::work(&cli.project),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    }
}
