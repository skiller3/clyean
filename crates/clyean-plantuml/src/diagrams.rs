// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The fourteen official UML diagram types and the PlantUML skeleton for each.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagramType {
    /// File stem used under `.clyean/architecture`, e.g. `class` -> `class.puml`.
    pub file_stem: &'static str,
    pub title: &'static str,
    pub category: DiagramCategory,
    pub template: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramCategory {
    Structure,
    Behavior,
}

macro_rules! diagram {
    ($stem:literal, $title:literal, $category:ident) => {
        DiagramType {
            file_stem: $stem,
            title: $title,
            category: DiagramCategory::$category,
            template: include_str!(concat!("../templates/", $stem, ".puml")),
        }
    };
}

pub const DIAGRAM_TYPES: [DiagramType; 14] = [
    diagram!("class", "Class Diagram", Structure),
    diagram!("object", "Object Diagram", Structure),
    diagram!("package", "Package Diagram", Structure),
    diagram!("component", "Component Diagram", Structure),
    diagram!(
        "composite-structure",
        "Composite Structure Diagram",
        Structure
    ),
    diagram!("deployment", "Deployment Diagram", Structure),
    diagram!("profile", "Profile Diagram", Structure),
    diagram!("use-case", "Use Case Diagram", Behavior),
    diagram!("activity", "Activity Diagram", Behavior),
    diagram!("state-machine", "State Machine Diagram", Behavior),
    diagram!("sequence", "Sequence Diagram", Behavior),
    diagram!("communication", "Communication Diagram", Behavior),
    diagram!(
        "interaction-overview",
        "Interaction Overview Diagram",
        Behavior
    ),
    diagram!("timing", "Timing Diagram", Behavior),
];

impl DiagramType {
    pub fn source_file_name(&self) -> String {
        format!("{}.puml", self.file_stem)
    }

    pub fn pdf_file_name(&self) -> String {
        format!("{}.pdf", self.file_stem)
    }

    /// The template with the project name substituted into its title.
    pub fn render_template(&self, project_name: &str) -> String {
        self.template.replace("{{PROJECT_NAME}}", project_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_a_complete_plantuml_document() {
        for diagram in DIAGRAM_TYPES {
            let rendered = diagram.render_template("Example");
            assert!(rendered.starts_with("@startuml"), "{}", diagram.file_stem);
            assert!(
                rendered.trim_end().ends_with("@enduml"),
                "{}",
                diagram.file_stem
            );
            assert!(rendered.contains("Example"), "{}", diagram.file_stem);
            assert!(!rendered.contains("{{PROJECT_NAME}}"));
        }
        assert_eq!(DIAGRAM_TYPES.len(), 14);
    }
}
