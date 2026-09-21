// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Semantics of the local-only `*.local.<ext>` companions of tracked scaffold files.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Deep-merges `overlay` into `base`: objects merge key by key, every other value in the
/// overlay replaces the base value, and `null` in the overlay removes the key.
pub fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                if overlay_value.is_null() {
                    base_map.remove(&key);
                    continue;
                }
                let merged = match base_map.remove(&key) {
                    Some(base_value) => deep_merge(base_value, overlay_value),
                    None => overlay_value,
                };
                base_map.insert(key, merged);
            }
            Value::Object(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// The `*.local.<ext>` companion of a tracked file, e.g. `X.omp.json` -> `X.omp.local.json`.
pub fn local_companion(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let local_name = match file_name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}.local.{extension}"),
        None => format!("{file_name}.local"),
    };
    path.with_file_name(local_name)
}

/// Reads a tracked Markdown file and appends its local companion when present.
pub fn read_markdown_with_local_enhancement(path: &Path) -> std::io::Result<String> {
    let mut text = std::fs::read_to_string(path)?;
    let companion = local_companion(path);
    if companion.is_file() {
        let local = std::fs::read_to_string(&companion)?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
        text.push_str(&local);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deep_merge_merges_nested_objects_and_replaces_scalars() {
        let base = json!({"a": {"x": 1, "y": 2}, "b": [1, 2], "c": "keep"});
        let overlay = json!({"a": {"y": 3, "z": 4}, "b": [9], "d": true});
        assert_eq!(
            deep_merge(base, overlay),
            json!({"a": {"x": 1, "y": 3, "z": 4}, "b": [9], "c": "keep", "d": true})
        );
    }

    #[test]
    fn deep_merge_null_removes_a_key() {
        let merged = deep_merge(json!({"a": 1, "b": 2}), json!({"a": null}));
        assert_eq!(merged, json!({"b": 2}));
    }

    #[test]
    fn local_companion_inserts_local_before_the_extension() {
        assert_eq!(
            local_companion(Path::new("/p/.clyean/agents/PROGRAMMER.omp.json")),
            PathBuf::from("/p/.clyean/agents/PROGRAMMER.omp.local.json")
        );
        assert_eq!(
            local_companion(Path::new("AGENTS__PROGRAMMER.md")),
            PathBuf::from("AGENTS__PROGRAMMER.local.md")
        );
        assert_eq!(
            local_companion(Path::new("LICENSE")),
            PathBuf::from("LICENSE.local")
        );
    }

    #[test]
    fn markdown_enhancement_is_appended_after_a_blank_line() {
        let dir = tempfile::tempdir().unwrap();
        let tracked = dir.path().join("AGENTS__X.md");
        std::fs::write(&tracked, "base").unwrap();
        assert_eq!(
            read_markdown_with_local_enhancement(&tracked).unwrap(),
            "base"
        );
        std::fs::write(dir.path().join("AGENTS__X.local.md"), "local\n").unwrap();
        assert_eq!(
            read_markdown_with_local_enhancement(&tracked).unwrap(),
            "base\n\nlocal\n"
        );
    }
}
