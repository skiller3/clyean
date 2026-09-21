// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Rendering of `*.puml` sources to PDF inside the sandbox.  Rendering is local only:
//! sources are never sent to a PlantUML server.  A non-zero PlantUML exit status is a
//! failed render whose output is discarded, because PlantUML can emit an image carrying
//! a sponsored message in place of a diagram when it fails.

use std::path::Path;

use crate::distribution::{LAYOUT_ENGINE_ARGUMENT, SANDBOX_JAR_LINK};

/// The result of running one command inside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Executes an argument vector inside the sandbox and reports its outcome.
pub trait CommandRunner {
    fn run(&self, argv: &[String]) -> std::io::Result<CommandOutcome>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderResult {
    Rendered,
    Failed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSource {
    pub source_file_name: String,
    pub result: RenderResult,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderReport {
    pub sources: Vec<RenderedSource>,
}

impl RenderReport {
    pub fn failures(&self) -> impl Iterator<Item = &RenderedSource> {
        self.sources
            .iter()
            .filter(|source| !matches!(source.result, RenderResult::Rendered))
    }

    pub fn all_succeeded(&self) -> bool {
        self.failures().next().is_none()
    }
}

/// Shell script that renders one source into a private temporary directory and moves the
/// PDF next to the source only when PlantUML exits successfully.
pub fn render_source_script(container_dir: &str, file_stem: &str) -> String {
    format!(
        "set -e\n\
         cd {dir}\n\
         tmp=$(mktemp -d)\n\
         trap 'rm -rf \"$tmp\"' EXIT\n\
         java -Djava.awt.headless=true -jar {jar} {layout} -tpdf -o \"$tmp\" {stem}.puml\n\
         mv \"$tmp/{stem}.pdf\" {stem}.pdf\n",
        dir = shell_quote(container_dir),
        jar = SANDBOX_JAR_LINK,
        layout = LAYOUT_ENGINE_ARGUMENT,
        stem = shell_quote(file_stem),
    )
}

/// Renders every `*.puml` file found in `host_dir`, executing PlantUML against the same
/// directory as seen from inside the sandbox at `container_dir`.
pub fn render_directory(
    runner: &dyn CommandRunner,
    host_dir: &Path,
    container_dir: &str,
) -> std::io::Result<RenderReport> {
    let mut stems: Vec<String> = std::fs::read_dir(host_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            name.strip_suffix(".puml").map(str::to_string)
        })
        .collect();
    stems.sort();
    let mut report = RenderReport::default();
    for stem in stems {
        let script = render_source_script(container_dir, &stem);
        let outcome = runner.run(&["sh".to_string(), "-c".to_string(), script])?;
        let result = if outcome.succeeded() {
            RenderResult::Rendered
        } else {
            RenderResult::Failed {
                exit_code: outcome.exit_code,
                stderr: outcome.stderr,
            }
        };
        report.sources.push(RenderedSource {
            source_file_name: format!("{stem}.puml"),
            result,
        });
    }
    Ok(report)
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRunner {
        calls: RefCell<Vec<Vec<String>>>,
        failing_stem: Option<&'static str>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, argv: &[String]) -> std::io::Result<CommandOutcome> {
            self.calls.borrow_mut().push(argv.to_vec());
            let script = &argv[2];
            let fails = self
                .failing_stem
                .is_some_and(|stem| script.contains(&format!("'{stem}'.puml")));
            Ok(CommandOutcome {
                exit_code: Some(if fails { 200 } else { 0 }),
                stdout: String::new(),
                stderr: if fails {
                    "syntax error".into()
                } else {
                    String::new()
                },
            })
        }
    }

    #[test]
    fn script_renders_into_a_temporary_directory_and_moves_on_success() {
        let script = render_source_script("/home/u/workspace/p/.clyean/architecture", "class");
        assert!(script.contains("mktemp -d"));
        assert!(script.contains("-Playout=smetana"));
        assert!(script.contains("-tpdf"));
        assert!(script.contains("mv \"$tmp/'class'.pdf\" 'class'.pdf"));
        assert!(script.contains("/opt/plantuml/plantuml.jar"));
        assert!(!script.contains("plantuml.com"));
    }

    #[test]
    fn report_flags_failed_sources_and_renders_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        for stem in ["class", "sequence", "notes.txt"] {
            std::fs::write(
                dir.path().join(format!("{stem}.puml")),
                "@startuml\n@enduml\n",
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("README.md"), "not a diagram").unwrap();
        let runner = FakeRunner {
            calls: RefCell::new(Vec::new()),
            failing_stem: Some("sequence"),
        };
        let report = render_directory(&runner, dir.path(), "/container/arch").unwrap();
        assert_eq!(report.sources.len(), 3);
        assert!(!report.all_succeeded());
        let failed: Vec<_> = report
            .failures()
            .map(|s| s.source_file_name.as_str())
            .collect();
        assert_eq!(failed, vec!["sequence.puml"]);
        assert_eq!(runner.calls.borrow().len(), 3);
        assert_eq!(runner.calls.borrow()[0][0], "sh");
    }
}
