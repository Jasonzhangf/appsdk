//! Independent, local project-memory index.
//!
//! Durable memory is human-readable JSONL under `memory/`. SQLite is a
//! rebuildable projection. The module intentionally has no knowledge of Guide
//! domains or AppSDK lifecycle state; `review` is the only run-note write-back
//! bridge, while `migrate` is the explicit source-preserving schema bridge.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CATEGORIES: [&str; 4] = ["plan", "path", "knowledge", "lesson"];
const MEMORY_SCHEMA_VERSION: i64 = 1;
const MIGRATION_RECORD: &str = "memory/migration.json";
const LEGACY_MEMORY_SOURCES: [&str; 2] = ["memory/entries.jsonl", "memory/memories.jsonl"];
const DETAIL_MARKER_PREFIX: &str = "<!-- project-memory:v1 ";
const DETAIL_END_MARKER: &str = "<!-- project-memory:end -->";
const SKILL_DESCRIPTION_BASE_SLOTS: usize = 8;
#[allow(dead_code)]
const INDEX_TEMPLATE: &str = include_str!("../../memory/index.md");

#[allow(dead_code)]
pub fn initialize_project(root: &Path) -> Result<(), &'static str> {
    let directory = root.join("memory");
    if fs::symlink_metadata(&directory)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("MEMORY_DIR_SYMLINK");
    }
    if directory.exists() && !directory.is_dir() {
        return Err("MEMORY_DIR_INVALID");
    }
    fs::create_dir_all(&directory).map_err(|_| "MEMORY_INDEX_WRITE_FAILED")?;
    let path = directory.join("index.md");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("MEMORY_INDEX_SYMLINK");
    }
    if !path.exists() {
        fs::write(path, INDEX_TEMPLATE).map_err(|_| "MEMORY_INDEX_WRITE_FAILED")?;
    }
    Ok(())
}

fn assert_memory_dir(root: &Path) {
    let directory = root.join("memory");
    if fs::symlink_metadata(&directory)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(
            "MEMORY_DIR_SYMLINK",
            "replace the memory directory with a project-owned directory",
        );
    }
    if directory.exists() && !directory.is_dir() {
        fail(
            "MEMORY_DIR_INVALID",
            "replace memory with a project-owned directory",
        );
    }
}

fn fail(code: &str, next: &str) -> ! {
    eprintln!(
        "{}",
        json!({"error": code, "retry_allowed": false, "next": next})
    );
    std::process::exit(1);
}

fn output(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

fn digest(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn assert_id(value: &str) {
    if !valid_id(value) {
        fail(
            "MEMORY_ID_INVALID",
            "use an alphanumeric, dash, underscore, dot, or slash ID",
        );
    }
}

fn category(value: &str) -> &str {
    if CATEGORIES.contains(&value) {
        value
    } else {
        fail(
            "MEMORY_CATEGORY_INVALID",
            "use plan, path, knowledge, or lesson",
        );
    }
}

fn memory_level(value: &Value) -> i64 {
    let candidate = value
        .get("memory_level")
        .or_else(|| value.get("level"))
        .or_else(|| value.get("layer"))
        .and_then(Value::as_i64)
        .filter(|level| (1..=3).contains(level))
        .unwrap_or(3);
    if review_status(value) == "reviewed" {
        candidate
    } else {
        3
    }
}

fn review_status(value: &Value) -> &str {
    let has_evidence = value
        .get("review_evidence")
        .and_then(Value::as_array)
        .is_some_and(|evidence| {
            evidence
                .iter()
                .any(|item| item.as_str().is_some_and(|item| !item.is_empty()))
        });
    match value.get("review_status").and_then(Value::as_str) {
        Some("reviewed") if has_evidence => "reviewed",
        _ => "unreviewed",
    }
}

fn detail_relative_path(id: &str, level: i64) -> String {
    let filename = id.replace('/', "--");
    format!("L{}/{}.md", level, filename)
}

fn detail_display_path(global: bool, id: &str, level: i64) -> String {
    if global {
        format!("global/{}", detail_relative_path(id, level))
    } else {
        format!("memory/{}", detail_relative_path(id, level))
    }
}

fn detail_path(root: &Path, global: bool, id: &str, level: i64) -> PathBuf {
    if global {
        home_dir()
            .join("global")
            .join(detail_relative_path(id, level))
    } else {
        memory_dir(root).join(detail_relative_path(id, level))
    }
}

fn markdown_heading(value: &str) -> String {
    value
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('#', "\\#")
}

fn write_detail(root: &Path, global: bool, value: &Value) {
    let id = entry_id(value);
    let level = memory_level(value);
    let metadata = json!({
        "id": id,
        "category": value.get("category").and_then(Value::as_str).unwrap_or("knowledge"),
        "tags": tags(value),
        "source_refs": refs(value),
        "memory_level": memory_level(value),
        "review_status": review_status(value),
        "review_evidence": value.get("review_evidence").cloned().unwrap_or_else(|| json!([])),
        "importance": value.get("importance").and_then(Value::as_i64).unwrap_or(0),
        "created_at": value.get("created_at").and_then(Value::as_str).unwrap_or(""),
        "updated_at": value.get("updated_at").and_then(Value::as_str).unwrap_or(""),
    });
    let title = value.get("title").and_then(Value::as_str).unwrap_or(&id);
    let content = entry_text(value);
    let mut text = format!(
        "{}{} -->\n\n# {}\n\n",
        DETAIL_MARKER_PREFIX,
        serde_json::to_string(&metadata).unwrap(),
        markdown_heading(title)
    );
    text.push_str(&content);
    if !content.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(DETAIL_END_MARKER);
    text.push('\n');
    atomic_write(&detail_path(root, global, &id, level), &text);
}

fn detail_directories(root: &Path, global: bool) -> Vec<(Option<i64>, PathBuf)> {
    let base = if global {
        home_dir().join("global")
    } else {
        memory_dir(root)
    };
    let mut directories = vec![(None, base.join("details"))];
    directories.extend((1..=3).map(|level| (Some(level), base.join(format!("L{level}")))));
    directories
}

fn parse_code_values(value: &str) -> Vec<String> {
    value
        .split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_detail(path: &Path) -> Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|_| fail("MEMORY_DETAIL_INVALID", "repair the Markdown detail file"));
    let mut metadata = None;
    if let Some(first) = text.lines().next() {
        if let Some(json_text) = first
            .strip_prefix(DETAIL_MARKER_PREFIX)
            .and_then(|value| value.strip_suffix(" -->"))
        {
            let value: Value = serde_json::from_str(json_text).unwrap_or_else(|_| {
                fail(
                    "MEMORY_DETAIL_INVALID",
                    "repair the project-memory metadata marker",
                )
            });
            if !value.is_object() {
                fail(
                    "MEMORY_DETAIL_INVALID",
                    "project-memory metadata must be a JSON object",
                );
            }
            metadata = Some(value);
        }
    }

    let mut heading = None;
    let mut body_start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_text = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .trim_end_matches('\r');
        if let Some(title) = line_text.strip_prefix("# ") {
            heading = Some(title.replace("\\#", "#"));
            body_start = Some(offset + line.len());
            break;
        }
        offset += line.len();
    }
    let Some(title) = heading else {
        fail(
            "MEMORY_DETAIL_INVALID",
            "add a Markdown level-one title to the detail file",
        )
    };
    let body_start = body_start.unwrap();
    let marked_end = text[body_start..]
        .split_inclusive('\n')
        .scan(body_start, |offset, line| {
            let line_start = *offset;
            *offset += line.len();
            Some((line_start, line))
        })
        .find_map(|(line_start, line)| {
            let line_text = line
                .strip_suffix('\n')
                .unwrap_or(line)
                .trim_end_matches('\r');
            (line_text == DETAIL_END_MARKER).then_some(line_start)
        });
    let (body_end, legacy_footer) = if let Some(end) = marked_end {
        (end, None)
    } else {
        let remainder = &text[body_start..];
        let separator = remainder.find("\n---\n").unwrap_or_else(|| {
            fail(
                "MEMORY_DETAIL_UNSUPPORTED",
                "use an exported project-memory detail or migrate the raw JSONL source",
            )
        });
        (
            body_start + separator,
            Some(&remainder[separator + "\n---\n".len()..]),
        )
    };
    let mut content = text[body_start..body_end].to_string();
    if content.starts_with('\n') {
        content.remove(0);
    }
    if legacy_footer.is_some() {
        content = content.trim_end_matches('\n').to_string();
    } else if content.ends_with('\n') {
        content.pop();
    }
    if content.is_empty() {
        fail(
            "MEMORY_ENTRY_CONTENT_REQUIRED",
            "add non-empty Markdown detail content",
        );
    }

    let mut value = metadata.unwrap_or_else(|| json!({}));
    if let Some(footer) = legacy_footer {
        for line in footer.lines() {
            if let Some(raw) = line.strip_prefix("- id: ") {
                if let Some(id) = parse_code_values(raw).first() {
                    value["id"] = Value::String(id.clone());
                }
            } else if let Some(raw) = line.strip_prefix("- tags: ") {
                value["tags"] = Value::Array(
                    parse_code_values(raw)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                );
            } else if let Some(raw) = line.strip_prefix("- source_refs: ") {
                value["source_refs"] = Value::Array(
                    parse_code_values(raw)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                );
            } else if let Some(raw) = line.strip_prefix("- memory_level: ") {
                if let Ok(level) = raw.trim().parse::<i64>() {
                    value["memory_level"] = Value::Number(level.into());
                }
            } else if let Some(raw) = line.strip_prefix("- review_status: ") {
                value["review_status"] = Value::String(raw.trim_matches('`').to_string());
            }
        }
    }
    if value.get("id").and_then(Value::as_str).is_none() {
        fail(
            "MEMORY_DETAIL_INVALID",
            "exported detail metadata must contain an id",
        );
    }
    value["title"] = Value::String(title);
    value["content"] = Value::String(content);
    value
}

fn read_detail_entries(root: &Path, global: bool) -> Vec<Value> {
    let mut legacy = BTreeMap::new();
    let mut canonical = BTreeMap::new();
    let expected_levels = effective_entries_for_scope(root, global)
        .into_iter()
        .map(|entry| (entry_id(&entry), memory_level(&entry)))
        .collect::<BTreeMap<_, _>>();
    for (level, directory) in detail_directories(root, global) {
        if fs::symlink_metadata(&directory)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(
                "MEMORY_DETAILS_SYMLINK",
                "replace the detail directory with a project-owned directory",
            );
        }
        if !directory.is_dir() {
            continue;
        }
        let mut paths = fs::read_dir(&directory)
            .unwrap_or_else(|_| fail("MEMORY_DETAILS_READ_FAILED", "repair memory details"))
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            if fs::symlink_metadata(&path)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                fail(
                    "MEMORY_DETAIL_SYMLINK",
                    "replace the detail file with a project-owned regular file",
                );
            }
            let value = parse_detail(&path);
            let id = entry_id(&value);
            if let Some(level) = level {
                if let Some((existing_level, _)) = canonical.get(&id) {
                    let expected = expected_levels.get(&id).copied();
                    let existing_is_expected = expected == Some(*existing_level);
                    let incoming_is_expected = expected == Some(level);
                    if existing_is_expected && !incoming_is_expected {
                        continue;
                    }
                    if incoming_is_expected && !existing_is_expected {
                        canonical.insert(id, (level, value));
                        continue;
                    }
                    fail(
                        "MEMORY_DETAIL_DUPLICATE_ID",
                        "keep one Markdown detail file per memory ID",
                    );
                }
                canonical.insert(id, (level, value));
            } else if legacy.insert(id, value).is_some() {
                fail(
                    "MEMORY_DETAIL_DUPLICATE_ID",
                    "keep one Markdown detail file per memory ID",
                );
            }
        }
    }
    for (id, (_, value)) in canonical {
        legacy.insert(id, value);
    }
    legacy.into_values().collect()
}

fn import_details(root: &Path, global: bool) -> Value {
    assert_memory_dir(root);
    let mut entries = read_detail_entries(root, global);
    let existing = effective_entries_for_scope(root, global);
    let mut ids = BTreeSet::new();
    for entry in &mut entries {
        let id = entry_id(entry);
        assert_id(&id);
        if !ids.insert(id.clone()) {
            fail(
                "MEMORY_DETAIL_DUPLICATE_ID",
                "keep one Markdown detail file per memory ID",
            );
        }
        if entry.get("category").and_then(Value::as_str).is_none() {
            let inferred = existing
                .iter()
                .find(|existing| entry_id(existing) == id)
                .and_then(|existing| {
                    existing
                        .get("category")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| "knowledge".to_string());
            entry["category"] = Value::String(inferred);
        }
        let entry_category = entry
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                fail(
                    "MEMORY_DETAIL_INVALID",
                    "detail metadata category must be a string",
                )
            });
        category(entry_category);
        if let Some(previous) = existing.iter().find(|previous| entry_id(previous) == id) {
            if previous.get("category").and_then(Value::as_str) != Some(entry_category) {
                fail(
                    "MEMORY_CATEGORY_CHANGE",
                    "keep one category per memory ID or create a new ID",
                );
            }
        }
    }
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for entry in entries {
        let result = write_entry(root, global, entry);
        if result.get("deduplicated") == Some(&Value::Bool(true)) {
            skipped += 1;
        } else {
            imported += 1;
        }
    }
    json!({
        "status": if imported == 0 { "already_current" } else { "complete" },
        "scope": if global { "global" } else { "project" },
        "details_found": imported + skipped,
        "imported_entries": imported,
        "skipped_existing": skipped,
        "raw_sources_retained": true,
        "index_updated": true
    })
}

fn write_memory_index(root: &Path, global: bool, entries: &[Value]) -> PathBuf {
    let path = if global {
        home_dir().join("global/index.md")
    } else {
        memory_dir(root).join("index.md")
    };
    let mut text = String::from("# Memory Index\n\n");
    text.push_str(
        "Index stores short titles, tags, and detail paths. Open the linked Markdown detail for content.\n\n",
    );
    text.push_str("## Raw sources\n\n");
    for category in CATEGORIES {
        text.push_str(&format!(
            "- [{}]({}.jsonl)\n",
            category[..1].to_uppercase() + &category[1..],
            category
        ));
    }
    text.push('\n');
    for level in 1..=3 {
        let label = match level {
            1 => "reviewed critical",
            2 => "reviewed reusable",
            _ => "new or unreviewed",
        };
        text.push_str(&format!("## Level {} — {}\n\n", level, label));
        let mut count = 0;
        for entry in entries.iter().filter(|entry| memory_level(entry) == level) {
            let id = entry_id(entry);
            let title = entry.get("title").and_then(Value::as_str).unwrap_or(&id);
            let detail = detail_relative_path(&id, level);
            text.push_str(&format!(
                "### {}\n- tags: {}\n- details: [{}]({})\n\n",
                markdown_heading(title),
                if tags(entry).is_empty() {
                    "_none_".to_string()
                } else {
                    tags(entry)
                        .into_iter()
                        .map(|tag| format!("`{tag}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                detail,
                detail
            ));
            count += 1;
        }
        if count == 0 {
            text.push_str("_Empty._\n\n");
        }
    }
    text.push_str("## Skill description candidates\n\n");
    text.push_str(
        "Base budget: 8 lines. Copy the compact lines into the project Skill `description` after manual architecture deduplication. Fill level 1 first; use level 2, then level 3, only for remaining slots.\n\n",
    );
    let mut candidate_count = 0;
    for level in 1..=3 {
        for entry in entries.iter().filter(|entry| memory_level(entry) == level) {
            if candidate_count >= SKILL_DESCRIPTION_BASE_SLOTS {
                break;
            }
            let id = entry_id(entry);
            let title = entry.get("title").and_then(Value::as_str).unwrap_or(&id);
            let kind = entry
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("knowledge");
            let tag_text = if tags(entry).is_empty() {
                "no-tag".to_string()
            } else {
                tags(entry).join(",")
            };
            text.push_str(&format!(
                "- L{}: {} ({}) [{}] -> {}\n",
                level,
                markdown_heading(title),
                kind,
                tag_text,
                detail_relative_path(&id, level)
            ));
            candidate_count += 1;
        }
        if candidate_count >= SKILL_DESCRIPTION_BASE_SLOTS {
            break;
        }
    }
    if candidate_count == 0 {
        text.push_str("- _No reviewed or searchable entries yet._\n");
    }
    text.push('\n');
    atomic_write(&path, &text);
    path
}

fn home_dir() -> PathBuf {
    env::var_os("PROJECT_MEMORY_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/project-memory"))
        })
        .unwrap_or_else(|| fail("MEMORY_HOME_UNAVAILABLE", "set PROJECT_MEMORY_HOME"))
}

fn project_id(root: &Path) -> String {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| fail("MEMORY_PROJECT_MISSING", "use an existing project root"));
    digest(&canonical.to_string_lossy())[7..23].to_string()
}

fn db_path(root: &Path, global: bool) -> PathBuf {
    if global {
        home_dir().join("global/index.sqlite")
    } else {
        home_dir()
            .join("projects")
            .join(project_id(root))
            .join("index.sqlite")
    }
}

fn memory_dir(root: &Path) -> PathBuf {
    root.join("memory")
}

fn category_file(root: &Path, category: &str) -> PathBuf {
    memory_dir(root).join(format!("{}.jsonl", category))
}

fn migration_record_path(root: &Path) -> PathBuf {
    root.join(MIGRATION_RECORD)
}

fn read_migration_record(root: &Path) -> Option<Value> {
    let path = migration_record_path(root);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(
            "MEMORY_MIGRATION_RECORD_SYMLINK",
            "replace memory/migration.json with a project-owned regular file",
        );
    }
    if !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(path).unwrap_or_else(|_| {
        fail(
            "MEMORY_MIGRATION_RECORD_INVALID",
            "repair memory/migration.json",
        )
    });
    Some(serde_json::from_str(&text).unwrap_or_else(|_| {
        fail(
            "MEMORY_MIGRATION_RECORD_INVALID",
            "repair memory/migration.json",
        )
    }))
}

fn write_migration_record(root: &Path, record: &Value) {
    atomic_write(
        &migration_record_path(root),
        &(serde_json::to_string_pretty(record).unwrap() + "\n"),
    );
}

fn legacy_source(root: &Path) -> Option<PathBuf> {
    for relative in LEGACY_MEMORY_SOURCES {
        let path = root.join(relative);
        if fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(
                "MEMORY_MIGRATION_SOURCE_SYMLINK",
                "replace the legacy memory source with a project-owned regular file",
            );
        }
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn read_jsonl_source(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_else(|_| {
            fail(
                "MEMORY_MIGRATION_SOURCE_INVALID",
                "repair the legacy memory JSONL source",
            )
        })
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).unwrap_or_else(|_| {
                fail(
                    "MEMORY_MIGRATION_SOURCE_INVALID",
                    &format!("repair legacy memory JSONL line {}", index + 1),
                )
            })
        })
        .collect()
}

fn normalized_migration_entry(value: &Value, source: &str, line: usize) -> Value {
    let id = entry_id(value);
    assert_id(&id);
    let cat = category(
        value
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("knowledge"),
    )
    .to_string();
    let content = entry_text(value);
    let source_ref = format!("{}#{}", source, line + 1);
    let mut normalized = value.clone();
    normalized["id"] = Value::String(id);
    normalized["category"] = Value::String(cat);
    normalized["content"] = Value::String(content);
    normalized["tags"] = Value::Array(tags(value).into_iter().map(Value::String).collect());
    let mut source_refs = refs(value);
    if !source_refs.contains(&source_ref) {
        source_refs.push(source_ref);
    }
    source_refs.sort();
    source_refs.dedup();
    normalized["source_refs"] = Value::Array(source_refs.into_iter().map(Value::String).collect());
    normalized
}

fn migration(root: &Path) -> Value {
    assert_memory_dir(root);
    let source = legacy_source(root);
    let source_path = source
        .as_ref()
        .map(|path| path.to_string_lossy().to_string());
    let source_digest = source.as_ref().map(|path| {
        let text = fs::read_to_string(path).unwrap_or_else(|_| {
            fail(
                "MEMORY_MIGRATION_SOURCE_INVALID",
                "repair the legacy memory JSONL source",
            )
        });
        digest(&text)
    });

    if let Some(previous) = read_migration_record(root) {
        if previous.get("target_schema").and_then(Value::as_i64) != Some(MEMORY_SCHEMA_VERSION) {
            fail(
                "MEMORY_MIGRATION_TARGET_UNSUPPORTED",
                "use a project-memory binary that supports this target schema",
            );
        }
        if previous.get("status").and_then(Value::as_str) == Some("complete") {
            if previous.get("source_digest").and_then(Value::as_str) != source_digest.as_deref() {
                fail(
                    "MEMORY_MIGRATION_SOURCE_CHANGED",
                    "preserve the old source and start an explicit migration for the new source",
                );
            }
            let index = sync_index(root, false);
            return json!({
                "status": "already_complete",
                "migration_id": previous.get("migration_id").cloned().unwrap_or_else(|| json!("project-memory-v0-to-v1")),
                "source": source_path,
                "source_digest": source_digest,
                "resumed": false,
                "raw_sources_retained": true,
                "index": index
            });
        }
        if matches!(
            previous.get("status").and_then(Value::as_str),
            Some("in_progress" | "failed")
        ) {
            if previous.get("source_digest").and_then(Value::as_str) != source_digest.as_deref() {
                fail(
                    "MEMORY_MIGRATION_SOURCE_CHANGED",
                    "preserve the old source and start an explicit migration for the new source",
                );
            }
        }
    }

    let Some(source) = source else {
        let record = json!({
            "schema_version": 1,
            "migration_id": "project-memory-v0-to-v1",
            "source_schema": "v1",
            "target_schema": MEMORY_SCHEMA_VERSION,
            "source": Value::Null,
            "source_digest": Value::Null,
            "status": "complete",
            "action": "noop",
            "raw_sources_retained": true,
            "updated_at": now()
        });
        write_migration_record(root, &record);
        let index = sync_index(root, false);
        return json!({"status":"not_needed","migration_id":"project-memory-v0-to-v1","source":null,"source_digest":null,"resumed":false,"raw_sources_retained":true,"index":index});
    };

    let source = source.to_string_lossy().to_string();
    let legacy = read_jsonl_source(Path::new(&source));
    let normalized = legacy
        .iter()
        .enumerate()
        .map(|(line, value)| normalized_migration_entry(value, &source, line))
        .collect::<Vec<_>>();
    let existing = effective_entries_for_scope(root, false);
    let conflicts = normalized
        .iter()
        .filter_map(|value| {
            let id = entry_id(value);
            existing
                .iter()
                .find(|entry| entry_id(entry) == id)
                .and_then(|entry| {
                    (entry.get("category") != value.get("category")
                        || entry.get("content") != value.get("content"))
                    .then_some(id)
                })
        })
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        let record = json!({
            "schema_version": 1,
            "migration_id": "project-memory-v0-to-v1",
            "source_schema": "legacy-flat-v0",
            "target_schema": MEMORY_SCHEMA_VERSION,
            "source": source.clone(),
            "source_digest": source_digest.clone(),
            "status": "blocked",
            "conflicts": conflicts,
            "raw_sources_retained": true,
            "updated_at": now()
        });
        write_migration_record(root, &record);
        return json!({
            "status": "blocked",
            "migration_id": "project-memory-v0-to-v1",
            "source": source,
            "source_digest": source_digest,
            "resumed": false,
            "conflicts": record["conflicts"],
            "raw_sources_retained": true,
            "next": "resolve conflicts or choose a new memory ID"
        });
    }
    let resumed = read_migration_record(root)
        .as_ref()
        .is_some_and(|record| record.get("status").and_then(Value::as_str) == Some("in_progress"));
    let mut record = json!({
        "schema_version": 1,
        "migration_id": "project-memory-v0-to-v1",
        "source_schema": "legacy-flat-v0",
        "target_schema": MEMORY_SCHEMA_VERSION,
        "source": source.clone(),
        "source_digest": source_digest.clone(),
        "status": "in_progress",
        "processed_entries": 0,
        "total_entries": normalized.len(),
        "raw_sources_retained": true,
        "updated_at": now()
    });
    write_migration_record(root, &record);

    let mut migrated = 0usize;
    let mut skipped_existing = 0usize;
    for value in normalized {
        let id = entry_id(&value);
        let existing = all_entries(root)
            .into_iter()
            .find(|entry| entry_id(entry) == id);
        if let Some(existing) = existing {
            if existing.get("category") == value.get("category")
                && existing.get("content") == value.get("content")
            {
                let result = write_entry(root, false, value);
                if result.get("deduplicated") == Some(&Value::Bool(true)) {
                    skipped_existing += 1;
                } else {
                    migrated += 1;
                }
            }
        } else {
            let _ = write_entry(root, false, value);
            migrated += 1;
        }
        record["processed_entries"] = Value::Number((migrated + skipped_existing).into());
        record["updated_at"] = Value::String(now());
        write_migration_record(root, &record);
    }
    let compacted = compact(root);
    record["status"] = Value::String("complete".into());
    record["migrated_entries"] = Value::Number(migrated.into());
    record["skipped_existing"] = Value::Number(skipped_existing.into());
    record["conflicts"] = Value::Array(Vec::new());
    record["completed_at"] = Value::String(now());
    record["updated_at"] = Value::String(now());
    write_migration_record(root, &record);
    json!({
        "status": "complete",
        "migration_id": "project-memory-v0-to-v1",
        "source": source,
        "source_digest": source_digest,
        "resumed": resumed,
        "migrated_entries": migrated,
        "skipped_existing": skipped_existing,
        "conflicts": [],
        "raw_sources_retained": true,
        "index": compacted["index"].clone()
    })
}

fn atomic_write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        if fs::symlink_metadata(parent)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(
                "MEMORY_SOURCE_SYMLINK",
                "replace the memory source directory with a project-owned directory",
            );
        }
        fs::create_dir_all(parent)
            .unwrap_or_else(|_| fail("MEMORY_WRITE_FAILED", "repair memory permissions"));
    }
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(
            "MEMORY_SOURCE_SYMLINK",
            "replace the memory source with a project-owned regular file",
        );
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = path.with_extension(format!("staging.{}.{}", std::process::id(), nonce));
    fs::write(&staging, text)
        .unwrap_or_else(|_| fail("MEMORY_WRITE_FAILED", "repair memory permissions"));
    fs::rename(staging, path)
        .unwrap_or_else(|_| fail("MEMORY_WRITE_FAILED", "repair memory permissions"));
}

fn read_entries(root: &Path, category: &str) -> Vec<Value> {
    assert_memory_dir(root);
    let path = category_file(root, category);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(
            "MEMORY_SOURCE_SYMLINK",
            "replace the memory source with a project-owned regular file",
        );
    }
    if !path.is_file() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .unwrap_or_else(|_| fail("MEMORY_SOURCE_INVALID", "repair the memory JSONL source"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|_| fail("MEMORY_SOURCE_INVALID", "repair the memory JSONL source"))
        })
        .collect()
}

fn all_entries(root: &Path) -> Vec<Value> {
    CATEGORIES
        .iter()
        .flat_map(|category| read_entries(root, category))
        .collect()
}

fn all_global_entries() -> Vec<Value> {
    let root = home_dir().join("global");
    CATEGORIES
        .iter()
        .flat_map(|category| {
            let path = root.join(format!("{}.jsonl", category));
            if !path.is_file() {
                return Vec::new();
            }
            fs::read_to_string(path)
                .unwrap_or_else(|_| {
                    fail(
                        "MEMORY_SOURCE_INVALID",
                        "repair the global memory JSONL source",
                    )
                })
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str(line).unwrap_or_else(|_| {
                        fail(
                            "MEMORY_SOURCE_INVALID",
                            "repair the global memory JSONL source",
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn union_array(previous: Option<&Value>, current: Option<&Value>) -> Option<Value> {
    let mut values = Vec::new();
    for source in [previous, current] {
        if let Some(array) = source.and_then(Value::as_array) {
            for value in array {
                if !values.iter().any(|existing| existing == value) {
                    values.push(value.clone());
                }
            }
        }
    }
    (!values.is_empty()).then_some(Value::Array(values))
}

/// Return one effective version per ID without destroying the append-only
/// source history. The last event owns content and timestamps; tags, source
/// references, and declared/semantic relations are monotonic unions.
fn effective_entries(entries: Vec<Value>) -> Vec<Value> {
    let mut effective = BTreeMap::<String, Value>::new();
    let mut categories = BTreeMap::<String, String>::new();
    for value in entries {
        let id = entry_id(&value);
        assert_id(&id);
        let current_category = category(
            value
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("knowledge"),
        )
        .to_string();
        if let Some(previous_category) = categories.get(&id) {
            if previous_category != &current_category {
                fail(
                    "MEMORY_CATEGORY_CHANGE",
                    "keep one category per memory ID or create a new ID",
                );
            }
        } else {
            categories.insert(id.clone(), current_category);
        }
        if let Some(previous) = effective.get(&id) {
            let mut merged = value.clone();
            for field in [
                "tags",
                "source_refs",
                "related_ids",
                "relations",
                "semantic_relations",
            ] {
                if let Some(union) = union_array(previous.get(field), value.get(field)) {
                    merged[field] = union;
                }
            }
            if merged.get("title").and_then(Value::as_str).is_none() {
                if let Some(title) = previous.get("title") {
                    merged["title"] = title.clone();
                }
            }
            if merged.get("created_at").and_then(Value::as_str).is_none() {
                if let Some(created_at) = previous.get("created_at") {
                    merged["created_at"] = created_at.clone();
                }
            }
            if merged.get("updated_at").and_then(Value::as_str).is_none() {
                if let Some(updated_at) = previous.get("updated_at") {
                    merged["updated_at"] = updated_at.clone();
                }
            }
            for field in [
                "memory_level",
                "review_status",
                "review_evidence",
                "detail_path",
            ] {
                if merged.get(field).is_none() {
                    if let Some(previous_value) = previous.get(field) {
                        merged[field] = previous_value.clone();
                    }
                }
            }
            effective.insert(id, merged);
        } else {
            effective.insert(id, value);
        }
    }
    effective.into_values().collect()
}

fn effective_entries_for_scope(root: &Path, global: bool) -> Vec<Value> {
    if global {
        effective_entries(all_global_entries())
    } else {
        effective_entries(all_entries(root))
    }
}

fn source_digest(root: &Path, global: bool) -> String {
    let base = if global {
        home_dir().join("global")
    } else {
        memory_dir(root)
    };
    let mut source = String::new();
    for category in CATEGORIES {
        let path = base.join(format!("{}.jsonl", category));
        if fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(
                "MEMORY_SOURCE_SYMLINK",
                "replace the memory source with a project-owned regular file",
            );
        }
        if path.is_file() {
            source.push_str(category);
            source.push('\0');
            source.push_str(&fs::read_to_string(path).unwrap_or_else(|_| {
                fail("MEMORY_SOURCE_INVALID", "repair the memory JSONL source")
            }));
            source.push('\0');
        }
    }
    digest(&source)
}

fn tags(value: &Value) -> Vec<String> {
    let mut tags = value
        .get("tags")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    tags.sort();
    tags.dedup();
    tags
}

fn refs(value: &Value) -> Vec<String> {
    let mut refs = value
        .get("source_refs")
        .or_else(|| value.get("sources"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    refs.sort();
    refs.dedup();
    refs
}

fn entry_id(value: &Value) -> String {
    value
        .get("id")
        .or_else(|| value.get("memory_id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("MEMORY_ENTRY_ID_REQUIRED", "declare id or memory_id"))
        .to_string()
}

fn entry_text(value: &Value) -> String {
    for key in [
        "content",
        "text",
        "summary",
        "lesson",
        "decision",
        "observation",
    ] {
        if let Some(text) = value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            return text.to_string();
        }
    }
    fail("MEMORY_ENTRY_CONTENT_REQUIRED", "declare content or text")
}

fn edge_values(value: &Value) -> Vec<(String, String, Option<f64>, Option<String>)> {
    let mut edges = Vec::new();
    for (key, relation) in [
        ("related_ids", "declared"),
        ("relations", "declared"),
        ("semantic_relations", "candidate_semantic"),
    ] {
        let Some(values) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for related in values {
            let (target, edge_type, score, model_revision) = if let Some(target) = related.as_str()
            {
                (target.to_string(), "related_to".to_string(), None, None)
            } else {
                let target = related
                    .get("to_id")
                    .or_else(|| related.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| fail("MEMORY_EDGE_INVALID", "declare a target memory ID"))
                    .to_string();
                (
                    target,
                    related
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("related_to")
                        .to_string(),
                    related.get("score").and_then(Value::as_f64),
                    related
                        .get("model_revision")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                )
            };
            assert_id(&target);
            edges.push((
                target,
                format!("{}:{}", relation, edge_type),
                score,
                model_revision.or_else(|| {
                    (relation == "candidate_semantic").then(|| "unconfigured".to_string())
                }),
            ));
        }
    }
    edges
}

fn init_schema(connection: &Connection) {
    connection
        .execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS memory_nodes (
               id TEXT PRIMARY KEY,
               category TEXT NOT NULL,
               title TEXT NOT NULL,
               content TEXT NOT NULL,
               tags_json TEXT NOT NULL,
               source_refs_json TEXT NOT NULL,
               importance INTEGER NOT NULL,
               layer INTEGER NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               project_id TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS memory_edges (
               from_id TEXT NOT NULL,
               to_id TEXT NOT NULL,
               relation TEXT NOT NULL,
               edge_type TEXT NOT NULL,
               source TEXT NOT NULL,
               score REAL,
               model_revision TEXT,
               PRIMARY KEY(from_id, to_id, relation, edge_type)
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS fts_entries USING fts5(
               id UNINDEXED, category, title, content, tags
             );
             CREATE TABLE IF NOT EXISTS vec_entries (
               id TEXT PRIMARY KEY,
               model_revision TEXT NOT NULL,
               dimension INTEGER NOT NULL,
               vector BLOB,
               content_hash TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS index_profile (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );",
        )
        .unwrap_or_else(|_| {
            fail(
                "MEMORY_INDEX_SCHEMA_FAILED",
                "repair the local SQLite runtime",
            )
        });
    let columns = connection
        .prepare("PRAGMA table_info(memory_nodes)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(Result::ok)
        .collect::<BTreeSet<_>>();
    if !columns.contains("review_status") {
        connection
            .execute(
                "ALTER TABLE memory_nodes ADD COLUMN review_status TEXT NOT NULL DEFAULT 'unreviewed'",
                [],
            )
            .unwrap();
    }
    if !columns.contains("detail_path") {
        connection
            .execute(
                "ALTER TABLE memory_nodes ADD COLUMN detail_path TEXT NOT NULL DEFAULT ''",
                [],
            )
            .unwrap();
    }
}

fn sync_index(root: &Path, global: bool) -> Value {
    let path = db_path(root, global);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| {
            fail(
                "MEMORY_INDEX_WRITE_FAILED",
                "repair memory index permissions",
            )
        });
    }
    let connection = Connection::open(&path).unwrap_or_else(|_| {
        fail(
            "MEMORY_INDEX_OPEN_FAILED",
            "repair the local SQLite runtime",
        )
    });
    init_schema(&connection);
    connection.execute("DELETE FROM memory_nodes", []).unwrap();
    connection.execute("DELETE FROM fts_entries", []).unwrap();
    connection.execute("DELETE FROM memory_edges", []).unwrap();
    let entries = effective_entries_for_scope(root, global);
    let source_digest = source_digest(root, global);
    for value in &entries {
        let id = entry_id(value);
        assert_id(&id);
        write_detail(root, global, value);
        let cat = category(
            value
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("knowledge"),
        );
        let content = entry_text(value);
        let title = value.get("title").and_then(Value::as_str).unwrap_or(&id);
        let tags = tags(value);
        let refs = refs(value);
        let importance = value
            .get("importance")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, 100);
        let layer = memory_level(value);
        let status = review_status(value);
        let detail = detail_display_path(global, &id, layer);
        let created = value
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        let updated = value
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or(created);
        connection.execute(
            "INSERT OR REPLACE INTO memory_nodes (id,category,title,content,tags_json,source_refs_json,importance,layer,created_at,updated_at,project_id,review_status,detail_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![id, cat, title, content, serde_json::to_string(&tags).unwrap(), serde_json::to_string(&refs).unwrap(), importance, layer, created, updated, if global { "global".to_string() } else { project_id(root) }, status, detail],
        ).unwrap_or_else(|_| fail("MEMORY_INDEX_WRITE_FAILED", "repair memory entry fields"));
        connection
            .execute(
                "INSERT INTO fts_entries (id,category,title,content,tags) VALUES (?1,?2,?3,?4,?5)",
                params![id, cat, title, content, tags.join(" ")],
            )
            .unwrap();
        for (target, edge_type, score, model_revision) in edge_values(value) {
            let relation = edge_type
                .split(':')
                .next()
                .unwrap_or("declared")
                .to_string();
            let edge_type = edge_type
                .split_once(':')
                .map(|(_, edge_type)| edge_type)
                .unwrap_or("related_to");
            connection
                .execute(
                    "INSERT OR REPLACE INTO memory_edges (from_id,to_id,relation,edge_type,source,score,model_revision) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![id, target, relation, edge_type, if relation == "candidate_semantic" { "wemm-adapter" } else { "memory-entry" }, score, model_revision],
                )
                .unwrap_or_else(|_| fail("MEMORY_INDEX_WRITE_FAILED", "repair a memory relation"));
        }
    }
    let index_path = write_memory_index(root, global, &entries);
    connection
        .execute(
            "INSERT OR REPLACE INTO index_profile (key,value) VALUES ('schema_version','2')",
            [],
        )
        .unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('semantic_backend','wemm-adapter')", []).unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('vector_backend','sqlite-vec-compatible schema; extension optional')", []).unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('semantic_status','candidate-only; inference-not-configured')", []).unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('query_order','anchors,exact,node,category,declared,lesson,fts,semantic,importance,updated_at')", []).unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO index_profile (key,value) VALUES ('source_digest',?1)",
            params![source_digest],
        )
        .unwrap();
    json!({"database": path, "index": index_path, "scope": if global { "global" } else { "project" }, "entries": entries.len(), "source_digest": source_digest, "semantic_backend": "wemm-adapter", "semantic_status": "candidate-only"})
}

fn generated_id(value: &Value) -> String {
    let title = value.get("title").and_then(Value::as_str).unwrap_or("");
    let content = entry_text(value);
    let seed = format!("{}\0{}\0{}", title, content, tags(value).join("\0"));
    format!("memory-{}", &digest(&seed)[7..23])
}

fn write_entry_with_review(
    root: &Path,
    global: bool,
    mut value: Value,
    reviewed: Option<(i64, Vec<String>)>,
) -> Value {
    let cat = category(
        value
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("knowledge"),
    )
    .to_string();
    if value.get("id").and_then(Value::as_str).is_none() {
        value["id"] = Value::String(generated_id(&value));
    }
    let id = entry_id(&value);
    assert_id(&id);
    let content = entry_text(&value);
    let timestamp = now();
    if value.get("title").and_then(Value::as_str).is_none() {
        value["title"] = Value::String(id.clone());
    }
    value["category"] = Value::String(cat.clone());
    value["content"] = Value::String(content);
    value["tags"] = Value::Array(tags(&value).into_iter().map(Value::String).collect());
    value["source_refs"] = Value::Array(refs(&value).into_iter().map(Value::String).collect());
    let current = effective_entries_for_scope(root, global)
        .into_iter()
        .find(|existing| entry_id(existing) == id);
    let is_review_write = reviewed.is_some();
    match reviewed {
        Some((level, evidence)) => {
            if !matches!(level, 1 | 2) || evidence.is_empty() {
                fail(
                    "MEMORY_REVIEW_INVALID",
                    "promote only to level 1 or 2 with review evidence",
                );
            }
            value["memory_level"] = Value::Number(level.into());
            value["review_status"] = Value::String("reviewed".into());
            value["review_evidence"] =
                Value::Array(evidence.into_iter().map(Value::String).collect());
        }
        None => {
            let level = current.as_ref().map(memory_level).unwrap_or(3);
            let status = current.as_ref().map(review_status).unwrap_or("unreviewed");
            value["memory_level"] = Value::Number(level.into());
            value["review_status"] = Value::String(status.into());
            if let Some(existing) = &current {
                if let Some(evidence) = existing.get("review_evidence") {
                    value["review_evidence"] = evidence.clone();
                }
            }
        }
    }
    value["layer"] = Value::Number(memory_level(&value).into());
    value["detail_path"] = Value::String(detail_display_path(global, &id, memory_level(&value)));
    value["created_at"] = value
        .get("created_at")
        .cloned()
        .unwrap_or_else(|| Value::String(timestamp.clone()));
    value["updated_at"] = Value::String(timestamp);
    let incoming_tags = tags(&value);
    let incoming_refs = refs(&value);
    if let Some(existing) = &current {
        if existing.get("category").and_then(Value::as_str) != Some(cat.as_str()) {
            fail(
                "MEMORY_CATEGORY_CHANGE",
                "keep one category per memory ID or create a new ID",
            );
        }
    }
    let duplicate = !is_review_write
        && current.as_ref().is_some_and(|existing| {
            existing.get("content").and_then(Value::as_str)
                == value.get("content").and_then(Value::as_str)
                && incoming_tags.iter().all(|tag| tags(existing).contains(tag))
                && incoming_refs
                    .iter()
                    .all(|source| refs(existing).contains(source))
        });
    if duplicate {
        let index = sync_index(root, global);
        return json!({"accepted": true, "deduplicated": true, "id": id, "category": cat, "index": index});
    }
    let path = if global {
        home_dir().join("global").join(format!("{}.jsonl", cat))
    } else {
        category_file(root, &cat)
    };
    if path.exists() {
        let mut text = fs::read_to_string(&path)
            .unwrap_or_else(|_| fail("MEMORY_SOURCE_INVALID", "repair the memory JSONL source"));
        text.push_str(&serde_json::to_string(&value).unwrap());
        text.push('\n');
        atomic_write(&path, &text);
    } else {
        atomic_write(&path, &(serde_json::to_string(&value).unwrap() + "\n"));
    }
    let index = sync_index(root, global);
    json!({"accepted": true, "write_mode": "one_shot", "id": id, "category": cat, "memory_level": memory_level(&value), "review_status": review_status(&value), "detail_path": detail_display_path(global, &id, memory_level(&value)), "index": index})
}

fn write_entry(root: &Path, global: bool, value: Value) -> Value {
    write_entry_with_review(root, global, value, None)
}

fn open_scope(root: &Path, global: bool) -> Connection {
    let path = db_path(root, global);
    if !path.is_file() {
        let _ = sync_index(root, global);
    }
    let connection = Connection::open(&path)
        .unwrap_or_else(|_| fail("MEMORY_INDEX_OPEN_FAILED", "run project-memory index"));
    init_schema(&connection);
    let recorded = connection
        .query_row(
            "SELECT value FROM index_profile WHERE key='source_digest'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .unwrap();
    let expected = source_digest(root, global);
    if recorded.as_deref() != Some(expected.as_str()) {
        drop(connection);
        let _ = sync_index(root, global);
        return Connection::open(path)
            .unwrap_or_else(|_| fail("MEMORY_INDEX_OPEN_FAILED", "run project-memory index"));
    }
    connection
}

fn query_scope(root: &Path, global: bool, query: &str) -> Vec<Value> {
    if query.trim().is_empty() {
        return all_scope(root, global);
    }
    let connection = open_scope(root, global);
    let pattern = query
        .split_whitespace()
        .map(|token| format!("{}*", token.replace('*', "").replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut statement = connection
        .prepare(
            "SELECT n.id,n.category,n.title,n.content,n.tags_json,n.importance,n.layer,n.updated_at,n.review_status,n.detail_path
         FROM memory_nodes n JOIN fts_entries f ON f.id=n.id
         WHERE fts_entries MATCH ?1 ORDER BY n.importance DESC,n.updated_at DESC LIMIT 50",
        )
        .unwrap();
    let rows = statement.query_map(params![pattern], |row| {
        let tags_json: String = row.get(4)?;
        Ok(json!({
            "id": row.get::<_, String>(0)?, "category": row.get::<_, String>(1)?,
            "title": row.get::<_, String>(2)?, "content": row.get::<_, String>(3)?,
            "tags": serde_json::from_str::<Value>(&tags_json).unwrap_or(json!([])),
            "importance": row.get::<_, i64>(5)?, "layer": row.get::<_, i64>(6)?,
            "memory_level": row.get::<_, i64>(6)?,
            "review_status": row.get::<_, String>(8)?,
            "detail_path": row.get::<_, String>(9)?,
            "updated_at": row.get::<_, String>(7)?, "scope": if global { "global" } else { "project" }
        }))
    }).unwrap_or_else(|_| fail("MEMORY_QUERY_FAILED", "use a plain-text query"));
    rows.filter_map(Result::ok).collect()
}

fn all_scope(root: &Path, global: bool) -> Vec<Value> {
    let connection = open_scope(root, global);
    let mut statement = connection
        .prepare(
            "SELECT id,category,title,content,tags_json,importance,layer,updated_at,review_status,detail_path
             FROM memory_nodes ORDER BY importance DESC,updated_at DESC LIMIT 50",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            let tags_json: String = row.get(4)?;
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "category": row.get::<_, String>(1)?,
                "title": row.get::<_, String>(2)?,
                "content": row.get::<_, String>(3)?,
                "tags": serde_json::from_str::<Value>(&tags_json).unwrap_or(json!([])),
                "importance": row.get::<_, i64>(5)?,
                "layer": row.get::<_, i64>(6)?,
                "memory_level": row.get::<_, i64>(6)?,
                "review_status": row.get::<_, String>(8)?,
                "detail_path": row.get::<_, String>(9)?,
                "updated_at": row.get::<_, String>(7)?,
                "scope": if global { "global" } else { "project" }
            }))
        })
        .unwrap_or_else(|_| fail("MEMORY_QUERY_FAILED", "repair the memory index"));
    rows.filter_map(Result::ok).collect()
}

fn edge_scope(root: &Path, global: bool) -> Vec<Value> {
    let connection = open_scope(root, global);
    let mut statement = connection
        .prepare(
            "SELECT from_id,to_id,relation,edge_type,source,score,model_revision FROM memory_edges",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok(json!({
                "from_id": row.get::<_, String>(0)?,
                "to_id": row.get::<_, String>(1)?,
                "relation": row.get::<_, String>(2)?,
                "type": row.get::<_, String>(3)?,
                "source": row.get::<_, String>(4)?,
                "score": row.get::<_, Option<f64>>(5)?,
                "model_revision": row.get::<_, Option<String>>(6)?
            }))
        })
        .unwrap_or_else(|_| fail("MEMORY_QUERY_FAILED", "repair the memory relation index"));
    rows.filter_map(Result::ok).collect()
}

fn filter_tags(entries: Vec<Value>, wanted: &[String]) -> Vec<Value> {
    if wanted.is_empty() {
        return entries;
    }
    entries
        .into_iter()
        .filter(|entry| {
            let actual = tags(entry);
            wanted
                .iter()
                .all(|wanted| actual.iter().any(|tag| tag == wanted))
        })
        .collect()
}

fn query(root: &Path, text: &str, wanted_tags: &[String]) -> Value {
    let mut indexed = all_scope(root, false);
    indexed.extend(all_scope(root, true));
    let exact = if valid_id(text) {
        get(root, text)
            .get("matches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let exact = filter_tags(exact, wanted_tags);
    let project = filter_tags(query_scope(root, false, text), wanted_tags);
    let global = filter_tags(query_scope(root, true, text), wanted_tags);
    let mut all = project.clone();
    all.extend(global.clone());
    let anchors = indexed
        .iter()
        .filter(|entry| entry["layer"] == 1)
        .cloned()
        .collect::<Vec<_>>();
    let mut matched_ids = exact
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    matched_ids.extend(
        all.iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .map(ToOwned::to_owned),
    );
    let mut edges = edge_scope(root, false);
    edges.extend(edge_scope(root, true));
    let related = edges
        .into_iter()
        .filter(|edge| {
            edge.get("from_id")
                .and_then(Value::as_str)
                .is_some_and(|id| matched_ids.contains(id))
                || edge
                    .get("to_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| matched_ids.contains(id))
        })
        .collect::<Vec<_>>();
    let declared_related = related
        .iter()
        .filter(|edge| edge["relation"] == "declared")
        .cloned()
        .collect::<Vec<_>>();
    let semantic_related = related
        .iter()
        .filter(|edge| {
            edge["relation"] == "candidate_semantic" || edge["relation"] == "accepted_semantic"
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut categories = CATEGORIES
        .iter()
        .map(|category| (*category).to_string())
        .collect::<BTreeSet<_>>();
    let mut tags = BTreeSet::new();
    for entry in &all {
        if let Some(cat) = entry["category"].as_str() {
            categories.insert(cat.to_string());
        }
        if let Some(values) = entry["tags"].as_array() {
            for tag in values.iter().filter_map(Value::as_str) {
                tags.insert(tag.to_string());
            }
        }
    }
    let next_queries = categories
        .into_iter()
        .chain(tags)
        .take(8)
        .map(Value::String)
        .collect::<Vec<_>>();
    json!({
        "query": text,
        "tags": wanted_tags,
        "anchors": anchors,
        "exact_matches": exact,
        "node_matches": [],
        "category_matches": all.clone(),
        "declared_related": declared_related,
        "lesson_matches": all.iter().filter(|entry| entry["category"] == "lesson").cloned().collect::<Vec<_>>(),
        "keyword_matches": all,
        "semantic_related": semantic_related,
        "semantic_backend": {"name":"WeMM-Embedding", "status":"candidate-only", "reason":"inference adapter is not configured"},
        "open_details": "use the detail_path returned for each match",
        "next_queries": next_queries
    })
}

fn get(root: &Path, id: &str) -> Value {
    assert_id(id);
    let mut matches = Vec::new();
    for global in [false, true] {
        let connection = open_scope(root, global);
        let mut statement = connection.prepare("SELECT id,category,title,content,tags_json,source_refs_json,importance,layer,created_at,updated_at,review_status,detail_path FROM memory_nodes WHERE id=?1").unwrap();
        let value = statement.query_row(params![id], |row| {
            let tags_json: String = row.get(4)?;
            let refs_json: String = row.get(5)?;
            Ok(json!({"id":row.get::<_,String>(0)?,"category":row.get::<_,String>(1)?,"title":row.get::<_,String>(2)?,"content":row.get::<_,String>(3)?,"tags":serde_json::from_str::<Value>(&tags_json).unwrap_or(json!([])),"source_refs":serde_json::from_str::<Value>(&refs_json).unwrap_or(json!([])),"importance":row.get::<_,i64>(6)?,"layer":row.get::<_,i64>(7)?,"memory_level":row.get::<_,i64>(7)?,"created_at":row.get::<_,String>(8)?,"updated_at":row.get::<_,String>(9)?,"review_status":row.get::<_,String>(10)?,"detail_path":row.get::<_,String>(11)?,"scope":if global {"global"} else {"project"}}))
        }).optional().unwrap();
        if let Some(value) = value {
            matches.push(value);
        }
    }
    json!({"id": id, "matches": matches})
}

fn review(root: &Path, run_id: &str) -> Value {
    assert_id(run_id);
    let path = root
        .join(".agent-collab/runs")
        .join(run_id)
        .join("notes.jsonl");
    if !path.is_file() {
        fail(
            "MEMORY_RUN_NOT_FOUND",
            "provide a completed run ID with notes.jsonl",
        );
    }
    let mut updates = Vec::new();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|_| fail("MEMORY_RUN_INVALID", "repair the run notes"));
    for (index, line) in text.lines().enumerate() {
        let note: Value = serde_json::from_str(line)
            .unwrap_or_else(|_| fail("MEMORY_RUN_INVALID", "repair the run notes JSONL"));
        let Some(memory) = note.get("memory").or_else(|| note.get("memory_update")) else {
            continue;
        };
        let mut value = memory.clone();
        let cat = value
            .get("category")
            .and_then(Value::as_str)
            .or_else(|| note.get("category").and_then(Value::as_str))
            .unwrap_or("lesson");
        category(cat);
        value["category"] = Value::String(cat.to_string());
        if value.get("id").is_none() {
            value["id"] = Value::String(format!("{}-{}", run_id, index + 1));
        }
        if value.get("source_refs").is_none() {
            value["source_refs"] = json!([format!(
                ".agent-collab/runs/{}/notes.jsonl#{}",
                run_id,
                index + 1
            )]);
        }
        let review = note
            .get("memory_review")
            .or_else(|| memory.get("memory_review"));
        if let Some(review) = review {
            let status = review.get("status").and_then(Value::as_str);
            let level = review.get("level").and_then(Value::as_i64);
            let evidence = review
                .get("evidence")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if status == Some("reviewed") && matches!(level, Some(1 | 2)) && !evidence.is_empty() {
                value["memory_review"] =
                    json!({"status":"reviewed","level":level,"evidence":evidence});
            }
        }
        updates.push(value);
    }
    if updates.is_empty() {
        return json!({"status":"no_update","run_id":run_id,"checked":true,"reason":"no explicit memory candidates in run notes","index_updated":false});
    }
    let mut result = Vec::new();
    for value in updates {
        let policy = value.get("memory_review").and_then(|review| {
            let level = review.get("level").and_then(Value::as_i64)?;
            let evidence = review
                .get("evidence")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            Some((level, evidence))
        });
        result.push(write_entry_with_review(root, false, value, policy));
    }
    json!({"status":"multiple_updates","run_id":run_id,"checked":true,"updates":result,"index_updated":true})
}

fn promote(root: &Path, id: &str, level: i64, evidence: Vec<String>) -> Value {
    assert_id(id);
    if !matches!(level, 1 | 2) || evidence.is_empty() {
        fail(
            "MEMORY_REVIEW_INVALID",
            "use level 1 or 2 with at least one review evidence reference",
        );
    }
    let current = effective_entries_for_scope(root, false)
        .into_iter()
        .find(|entry| entry_id(entry) == id)
        .unwrap_or_else(|| {
            fail(
                "MEMORY_ENTRY_NOT_FOUND",
                "query or create the memory before promotion",
            )
        });
    let mut next = current;
    next["memory_review"] = json!({"status":"reviewed","level":level,"evidence":evidence});
    let result = write_entry_with_review(root, false, next, Some((level, evidence)));
    json!({"status":"reviewed","id":id,"level":level,"result":result})
}

fn compact(root: &Path) -> Value {
    let entries = effective_entries_for_scope(root, false);
    let mut counts = BTreeMap::new();
    for category in CATEGORIES {
        counts.insert(
            category,
            entries
                .iter()
                .filter(|entry| entry.get("category").and_then(Value::as_str) == Some(category))
                .count(),
        );
    }
    let index = sync_index(root, false);
    json!({"accepted":true,"compressed":true,"raw_sources_retained":true,"tag_union_preserved":true,"source_events_unchanged":true,"categories":counts,"index":index})
}

fn verify(root: &Path) -> Value {
    let mut scopes = Vec::new();
    let mut ok = true;
    for global in [false, true] {
        let path = db_path(root, global);
        let expected_entries = effective_entries_for_scope(root, global);
        let expected_digest = source_digest(root, global);
        let Some(connection) = path.is_file().then(|| {
            Connection::open(&path).unwrap_or_else(|_| {
                fail(
                    "MEMORY_INDEX_OPEN_FAILED",
                    "repair the local SQLite runtime",
                )
            })
        }) else {
            let empty_scope = expected_entries.is_empty();
            ok &= empty_scope;
            scopes.push(json!({"scope":if global {"global"} else {"project"},"status":if empty_scope {"absent"} else {"missing"},"nodes":0,"fts_entries":0,"expected_nodes":expected_entries.len(),"source_consistent":empty_scope}));
            continue;
        };
        init_schema(&connection);
        let nodes: i64 = connection
            .query_row("SELECT count(*) FROM memory_nodes", [], |row| row.get(0))
            .unwrap();
        let fts: i64 = connection
            .query_row("SELECT count(*) FROM fts_entries", [], |row| row.get(0))
            .unwrap();
        let recorded_digest: Option<String> = connection
            .query_row(
                "SELECT value FROM index_profile WHERE key='source_digest'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let profile: Option<String> = connection
            .query_row(
                "SELECT value FROM index_profile WHERE key='query_order'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let source_consistent = recorded_digest.as_deref() == Some(expected_digest.as_str())
            && nodes == expected_entries.len() as i64
            && fts == expected_entries.len() as i64;
        ok &= source_consistent;
        scopes.push(json!({"scope":if global {"global"} else {"project"},"status":if source_consistent {"ready"} else {"stale"},"nodes":nodes,"fts_entries":fts,"expected_nodes":expected_entries.len(),"source_digest":expected_digest,"indexed_source_digest":recorded_digest,"source_consistent":source_consistent,"query_order":profile}));
    }
    json!({"ok":ok,"schema_version":1,"scopes":scopes,"categories":CATEGORIES,"semantic_backend":"wemm-adapter","semantic_status":"candidate-only","vector_backend":"sqlite-vec-compatible schema"})
}

fn run_notes(root: &Path, run_id: &str) -> (PathBuf, Vec<Value>) {
    assert_id(run_id);
    let path = root
        .join(".agent-collab/runs")
        .join(run_id)
        .join("notes.jsonl");
    if !path.is_file() {
        fail(
            "MEMORY_RUN_NOT_FOUND",
            "provide a run ID with notes.jsonl before re-entry",
        );
    }
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|_| fail("MEMORY_RUN_INVALID", "repair the run notes"));
    let notes = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).unwrap_or_else(|_| {
                fail(
                    "MEMORY_RUN_INVALID",
                    &format!("repair run notes JSONL line {}", index + 1),
                )
            })
        })
        .collect();
    (path, notes)
}

fn reentry(root: &Path, run_id: &str) -> Value {
    let (notes_path, notes) = run_notes(root, run_id);
    let migration = read_migration_record(root);
    let migration_status = migration
        .as_ref()
        .and_then(|record| record.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("missing");
    let migration_ready = migration_status == "complete";
    let index_path = db_path(root, false);
    let index_rebuilt = !index_path.is_file();
    if index_rebuilt {
        let _ = sync_index(root, false);
    }
    let last_note = notes.last().cloned().unwrap_or_else(|| json!({}));
    let mut resume_from = serde_json::Map::new();
    for key in [
        "node_id", "step_id", "event_id", "status", "stage", "result",
    ] {
        if let Some(value) = last_note.get(key) {
            resume_from.insert(key.to_string(), value.clone());
        }
    }
    if resume_from.is_empty() && !last_note.is_object() {
        resume_from.insert("last_note".into(), last_note.clone());
    }
    let mut next_queries = vec!["plan", "path", "knowledge", "lesson"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    for key in ["node_id", "step_id", "status", "stage"] {
        if let Some(value) = last_note.get(key).and_then(Value::as_str) {
            if !next_queries.iter().any(|query| query == value) {
                next_queries.push(value.to_string());
            }
        }
    }
    next_queries.truncate(8);
    let status = if migration_ready { "ready" } else { "blocked" };
    let next = if migration_ready {
        json!("query the returned anchors, then expand with next_queries")
    } else {
        json!("project-memory migrate [project] and re-enter with the same run ID")
    };
    json!({
        "status": status,
        "run_id": run_id,
        "notes": notes_path,
        "notes_count": notes.len(),
        "resume_from": Value::Object(resume_from),
        "last_note": last_note,
        "migration": {
            "status": migration_status,
            "ready": migration_ready,
            "record": migration.as_ref().and_then(|record| record.get("migration_id")).cloned()
        },
        "index": {"path": index_path, "rebuilt": index_rebuilt},
        "next_queries": next_queries,
        "next": next,
        "preserves_run_id": true
    })
}

fn parse_root(args: &mut Vec<String>) -> PathBuf {
    if args.first().is_some_and(|arg| !arg.starts_with('-')) {
        PathBuf::from(args.remove(0))
    } else {
        PathBuf::from(".")
    }
}

pub fn run(args: &mut impl Iterator<Item = String>) {
    let mut values = args.collect::<Vec<_>>();
    let command = values.first().cloned().unwrap_or_else(|| {
        fail(
            "MEMORY_USAGE",
            "select entry, query, get, review, promote, migrate, import, reentry, index, export, compact, or verify",
        )
    });
    values.remove(0);
    let root = if matches!(command.as_str(), "query" | "get") {
        PathBuf::from(".")
    } else {
        parse_root(&mut values)
    };
    match command.as_str() {
        "entry" => {
            let mut value = serde_json::Map::new();
            let mut global = false;
            let mut index = 0;
            while index < values.len() {
                let option = values[index].as_str();
                match option {
                    "--global" => {
                        global = true;
                        index += 1;
                    }
                    "--id" | "--category" | "--title" | "--text" | "--content" | "--importance"
                    | "--tag" => {
                        let arg = values.get(index + 1).cloned().unwrap_or_else(|| {
                            fail("MEMORY_USAGE", "provide a value after the entry option")
                        });
                        let key = option.trim_start_matches('-');
                        if key == "tag" {
                            let tags = value
                                .entry("tags")
                                .or_insert_with(|| json!([]))
                                .as_array_mut()
                                .unwrap();
                            tags.push(Value::String(arg));
                        } else if key == "text" || key == "content" {
                            value.insert("content".into(), Value::String(arg));
                        } else if key == "importance" {
                            value.insert(
                                key.into(),
                                Value::Number(arg.parse::<i64>().unwrap_or(0).into()),
                            );
                        } else {
                            value.insert(key.into(), Value::String(arg));
                        }
                        index += 2;
                    }
                    _ => fail(
                        "MEMORY_USAGE",
                        "use --id --category --title --text --tag [--global]",
                    ),
                }
            }
            output(&write_entry(&root, global, Value::Object(value)));
        }
        "query" => {
            let mut text_parts = Vec::new();
            let mut wanted_tags = Vec::new();
            let mut query_root = None;
            let mut index = 0;
            while index < values.len() {
                if values[index] == "--tag" {
                    wanted_tags.push(values.get(index + 1).cloned().unwrap_or_else(|| {
                        fail("MEMORY_USAGE", "provide a value after --tag")
                    }));
                    index += 2;
                } else if !values[index].starts_with('-') {
                    if text_parts.is_empty() {
                        text_parts.push(values[index].clone());
                    } else if query_root.is_none() {
                        query_root = Some(PathBuf::from(&values[index]));
                    } else {
                        fail("MEMORY_USAGE", "use query [text] [--tag <tag>] [project]");
                    }
                    index += 1;
                } else {
                    fail("MEMORY_USAGE", "use query [text] [--tag <tag>] [project]");
                }
            }
            if (text_parts.is_empty() || text_parts[0].trim().is_empty()) && wanted_tags.is_empty() {
                fail("MEMORY_QUERY_EMPTY", "provide text or at least one --tag");
            }
            let text = text_parts.first().cloned().unwrap_or_default();
            output(&query(
                query_root.as_deref().unwrap_or(&root),
                &text,
                &wanted_tags,
            ));
        }
        "get" => {
            let id = values
                .first()
                .cloned()
                .unwrap_or_else(|| fail("MEMORY_USAGE", "provide a memory ID"));
            let query_root = values
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| root.clone());
            if values.len() > 2 {
                fail(
                    "MEMORY_USAGE",
                    "use project-memory get <memory-id> [project]",
                );
            }
            output(&get(&query_root, &id));
        }
        "review" => {
            let mut run_id = None;
            let mut index = 0;
            while index < values.len() {
                if values[index] == "--run" {
                    run_id = values.get(index + 1).cloned();
                    index += 2;
                } else {
                    fail("MEMORY_USAGE", "use --run <run-id>");
                }
            }
            output(&review(
                &root,
                &run_id.unwrap_or_else(|| fail("MEMORY_USAGE", "provide --run <run-id>")),
            ));
        }
        "promote" => {
            let mut id = None;
            let mut level = None;
            let mut evidence = Vec::new();
            let mut index = 0;
            while index < values.len() {
                match values[index].as_str() {
                    "--id" | "--level" | "--evidence" => {
                        let arg = values.get(index + 1).cloned().unwrap_or_else(|| {
                            fail("MEMORY_USAGE", "provide a value after the promote option")
                        });
                        match values[index].as_str() {
                            "--id" => id = Some(arg),
                            "--level" => level = Some(arg.parse::<i64>().unwrap_or(0)),
                            "--evidence" => evidence.push(arg),
                            _ => unreachable!(),
                        }
                        index += 2;
                    }
                    _ => fail("MEMORY_USAGE", "use promote --id <id> --level <1|2> --evidence <ref>"),
                }
            }
            output(&promote(
                &root,
                &id.unwrap_or_else(|| fail("MEMORY_USAGE", "provide --id")),
                level.unwrap_or(0),
                evidence,
            ));
        }
        "migrate" => {
            if !values.is_empty() {
                fail("MEMORY_USAGE", "use project-memory migrate [project]");
            }
            output(&migration(&root));
        }
        "import" => {
            let mut global = false;
            for value in &values {
                if value == "--global" {
                    global = true;
                } else {
                    fail("MEMORY_USAGE", "use project-memory import [project] [--global]");
                }
            }
            output(&import_details(&root, global));
        }
        "reentry" | "resume" => {
            let mut run_id = None;
            let mut explicit_root = None;
            let mut index = 0;
            while index < values.len() {
                if values[index] == "--run" {
                    run_id = values.get(index + 1).cloned();
                    index += 2;
                } else if !values[index].starts_with('-') && explicit_root.is_none() {
                    explicit_root = Some(PathBuf::from(&values[index]));
                    index += 1;
                } else {
                    fail("MEMORY_USAGE", "use reentry [project] --run <run-id>");
                }
            }
            output(&reentry(
                explicit_root.as_deref().unwrap_or(&root),
                &run_id.unwrap_or_else(|| fail("MEMORY_USAGE", "provide --run <run-id>")),
            ));
        }
        "index" | "export" => {
            if !values.is_empty() {
                fail("MEMORY_USAGE", "use project-memory index|export [project]");
            }
            output(&json!({"project":sync_index(&root, false),"global":sync_index(&root, true)}));
        }
        "compact" => {
            if !values.is_empty() {
                fail("MEMORY_USAGE", "use project-memory compact [project]");
            }
            output(&compact(&root));
        }
        "verify" => {
            if !values.is_empty() {
                fail("MEMORY_USAGE", "use project-memory verify [project]");
            }
            output(&verify(&root));
        }
        "help" | "--help" | "-h" => output(
            &json!({"commands":["entry","query","get","review","promote","migrate","import","reentry","index","export","compact","verify"],"query_order":["level 1 titles","exact ID","node/function/resource","category/tag","declared relations/graph","lesson references","FTS5/tag","RAG candidates","importance","updated_at"],"query_hint":"project-memory query --tag <tag> [text] [project]; open detail_path directly; use export to render legacy/raw entries as Markdown; use import after intentionally editing an exported detail"}),
        ),
        _ => fail(
            "MEMORY_COMMAND_UNKNOWN",
            "select entry, query, get, review, promote, migrate, import, reentry, index, export, compact, or verify",
        ),
    }
}
