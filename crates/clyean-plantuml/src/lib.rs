// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The pinned MIT-licensed PlantUML distribution, the fourteen UML diagram templates that
//! seed `.clyean/architecture`, and local-only rendering of `*.puml` sources to PDF.

pub mod diagrams;
pub mod distribution;
pub mod render;

pub use diagrams::{DiagramType, DIAGRAM_TYPES};
pub use distribution::{PlantUmlDistribution, PINNED_DISTRIBUTION};
pub use render::{render_directory, CommandOutcome, CommandRunner, RenderReport, RenderResult};
