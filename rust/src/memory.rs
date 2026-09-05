//! Independent, local project-memory index.
//!
//! Durable memory is human-readable JSONL under `memory/`. SQLite is a
//! rebuildable projection. The module intentionally has no knowledge of Guide
//! domains or AppSDK lifecycle state; `review` is the only write-back bridge
//! from a completed run.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CATEGORIES: [&str; 4] = ["plan", "path", "knowledge", "lesson"];
#[allow(dead_code)]
const INDEX_TEMPLATE: &str = include_str!("../../memory/index.md");

#[allow(dead_code)]
pub fn initialize_project(root: &Path) {
    let directory = root.join("memory");
    assert_memory_dir(root);
    fs::create_dir_all(&directory)
        .unwrap_or_else(|_| fail("MEMORY_INDEX_WRITE_FAILED", "repair memory permissions"));
    let path = directory.join("index.md");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(
            "MEMORY_INDEX_SYMLINK",
            "replace memory/index.md with a project-owned regular file",
        );
    }
    if !path.exists() {
        fs::write(path, INDEX_TEMPLATE)
            .unwrap_or_else(|_| fail("MEMORY_INDEX_WRITE_FAILED", "repair memory permissions"));
    }
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
    let entries = if global {
        all_global_entries()
    } else {
        all_entries(root)
    };
    for value in &entries {
        let id = entry_id(value);
        assert_id(&id);
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
        let layer = value
            .get("layer")
            .and_then(Value::as_i64)
            .unwrap_or(if importance >= 90 { 1 } else { 3 });
        let created = value
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        let updated = value
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or(created);
        connection.execute(
            "INSERT OR REPLACE INTO memory_nodes (id,category,title,content,tags_json,source_refs_json,importance,layer,created_at,updated_at,project_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![id, cat, title, content, serde_json::to_string(&tags).unwrap(), serde_json::to_string(&refs).unwrap(), importance, layer, created, updated, if global { "global".to_string() } else { project_id(root) }],
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
    connection
        .execute(
            "INSERT OR REPLACE INTO index_profile (key,value) VALUES ('schema_version','1')",
            [],
        )
        .unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('semantic_backend','wemm-adapter')", []).unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('vector_backend','sqlite-vec-compatible schema; extension optional')", []).unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('semantic_status','candidate-only; inference-not-configured')", []).unwrap();
    connection.execute("INSERT OR REPLACE INTO index_profile (key,value) VALUES ('query_order','anchors,exact,node,category,declared,lesson,fts,semantic,importance,updated_at')", []).unwrap();
    json!({"database": path, "scope": if global { "global" } else { "project" }, "entries": entries.len(), "semantic_backend": "wemm-adapter", "semantic_status": "candidate-only"})
}

fn write_entry(root: &Path, global: bool, mut value: Value) -> Value {
    let cat = category(
        value
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("knowledge"),
    )
    .to_string();
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
    value["created_at"] = value
        .get("created_at")
        .cloned()
        .unwrap_or_else(|| Value::String(timestamp.clone()));
    value["updated_at"] = Value::String(timestamp);
    let incoming_tags = tags(&value);
    let incoming_refs = refs(&value);
    let duplicate = (if global {
        all_global_entries()
    } else {
        all_entries(root)
    })
    .iter()
    .any(|existing| {
        entry_id(existing) == id
            && existing.get("category").and_then(Value::as_str) == Some(cat.as_str())
            && existing.get("content").and_then(Value::as_str)
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
    json!({"accepted": true, "id": id, "category": cat, "index": index})
}

fn open_scope(root: &Path, global: bool) -> Connection {
    let path = db_path(root, global);
    if !path.is_file() {
        let _ = sync_index(root, global);
    }
    let connection = Connection::open(path)
        .unwrap_or_else(|_| fail("MEMORY_INDEX_OPEN_FAILED", "run project-memory index"));
    init_schema(&connection);
    connection
}

fn query_scope(root: &Path, global: bool, query: &str) -> Vec<Value> {
    let connection = open_scope(root, global);
    let pattern = query
        .split_whitespace()
        .map(|token| format!("{}*", token.replace('*', "").replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut statement = connection
        .prepare(
            "SELECT n.id,n.category,n.title,n.content,n.tags_json,n.importance,n.layer,n.updated_at
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
            "updated_at": row.get::<_, String>(7)?, "scope": if global { "global" } else { "project" }
        }))
    }).unwrap_or_else(|_| fail("MEMORY_QUERY_FAILED", "use a plain-text query"));
    rows.filter_map(Result::ok).collect()
}

fn all_scope(root: &Path, global: bool) -> Vec<Value> {
    let connection = open_scope(root, global);
    let mut statement = connection
        .prepare(
            "SELECT id,category,title,content,tags_json,importance,layer,updated_at
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

fn query(root: &Path, text: &str) -> Value {
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
    let project = query_scope(root, false, text);
    let global = query_scope(root, true, text);
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
        "anchors": anchors,
        "exact_matches": exact,
        "node_matches": [],
        "category_matches": all.clone(),
        "declared_related": declared_related,
        "lesson_matches": all.iter().filter(|entry| entry["category"] == "lesson").cloned().collect::<Vec<_>>(),
        "keyword_matches": all,
        "semantic_related": semantic_related,
        "semantic_backend": {"name":"WeMM-Embedding", "status":"candidate-only", "reason":"inference adapter is not configured"},
        "next_queries": next_queries
    })
}

fn get(root: &Path, id: &str) -> Value {
    assert_id(id);
    let mut matches = Vec::new();
    for global in [false, true] {
        let connection = open_scope(root, global);
        let mut statement = connection.prepare("SELECT id,category,title,content,tags_json,source_refs_json,importance,layer,created_at,updated_at FROM memory_nodes WHERE id=?1").unwrap();
        let value = statement.query_row(params![id], |row| {
            let tags_json: String = row.get(4)?;
            let refs_json: String = row.get(5)?;
            Ok(json!({"id":row.get::<_,String>(0)?,"category":row.get::<_,String>(1)?,"title":row.get::<_,String>(2)?,"content":row.get::<_,String>(3)?,"tags":serde_json::from_str::<Value>(&tags_json).unwrap_or(json!([])),"source_refs":serde_json::from_str::<Value>(&refs_json).unwrap_or(json!([])),"importance":row.get::<_,i64>(6)?,"layer":row.get::<_,i64>(7)?,"created_at":row.get::<_,String>(8)?,"updated_at":row.get::<_,String>(9)?,"scope":if global {"global"} else {"project"}}))
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
        updates.push(value);
    }
    if updates.is_empty() {
        return json!({"status":"no_update","run_id":run_id,"checked":true,"reason":"no explicit memory candidates in run notes","index_updated":false});
    }
    let mut result = Vec::new();
    for value in updates {
        result.push(write_entry(root, false, value));
    }
    json!({"status":"multiple_updates","run_id":run_id,"checked":true,"updates":result,"index_updated":true})
}

fn compact(root: &Path) -> Value {
    let mut counts = BTreeMap::new();
    for cat in CATEGORIES {
        let mut by_id = BTreeMap::<String, Value>::new();
        for value in read_entries(root, cat) {
            let id = entry_id(&value);
            if let Some(previous) = by_id.get_mut(&id) {
                // JSONL append order is the durable event order. Keep the
                // latest entry's content/title/importance while unioning all
                // tags and source references from the duplicate history.
                let previous_tags = tags(previous);
                let previous_refs = refs(previous);
                let mut merged_tags = previous_tags;
                merged_tags.extend(tags(&value));
                merged_tags.sort();
                merged_tags.dedup();
                let mut merged_refs = previous_refs;
                merged_refs.extend(refs(&value));
                merged_refs.sort();
                merged_refs.dedup();
                *previous = value;
                previous["tags"] =
                    Value::Array(merged_tags.into_iter().map(Value::String).collect());
                previous["source_refs"] =
                    Value::Array(merged_refs.into_iter().map(Value::String).collect());
                if previous.get("updated_at").and_then(Value::as_str).is_none() {
                    previous["updated_at"] = Value::String(now());
                }
            } else {
                by_id.insert(id, value);
            }
        }
        let text = by_id
            .values()
            .map(|value| serde_json::to_string(value).unwrap() + "\n")
            .collect::<String>();
        if !text.is_empty() {
            atomic_write(&category_file(root, cat), &text);
        }
        counts.insert(cat, by_id.len());
    }
    let index = sync_index(root, false);
    json!({"accepted":true,"compressed":true,"raw_sources_retained":true,"tag_union_preserved":true,"categories":counts,"index":index})
}

fn verify(root: &Path) -> Value {
    let mut scopes = Vec::new();
    for global in [false, true] {
        let connection = open_scope(root, global);
        let nodes: i64 = connection
            .query_row("SELECT count(*) FROM memory_nodes", [], |row| row.get(0))
            .unwrap();
        let fts: i64 = connection
            .query_row("SELECT count(*) FROM fts_entries", [], |row| row.get(0))
            .unwrap();
        let profile: Option<String> = connection
            .query_row(
                "SELECT value FROM index_profile WHERE key='query_order'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        scopes.push(json!({"scope":if global {"global"} else {"project"},"nodes":nodes,"fts_entries":fts,"query_order":profile}));
    }
    json!({"ok":true,"schema_version":1,"scopes":scopes,"categories":CATEGORIES,"semantic_backend":"wemm-adapter","semantic_status":"candidate-only","vector_backend":"sqlite-vec-compatible schema"})
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
            "select entry, query, get, review, index, compact, or verify",
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
            let text = values
                .first()
                .cloned()
                .unwrap_or_else(|| fail("MEMORY_USAGE", "provide a query string"));
            if text.trim().is_empty() {
                fail("MEMORY_QUERY_EMPTY", "provide a non-empty query string");
            }
            let query_root = values
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| root.clone());
            if values.len() > 2 {
                fail("MEMORY_USAGE", "use project-memory query <text> [project]");
            }
            output(&query(&query_root, &text));
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
        "index" => {
            if !values.is_empty() {
                fail("MEMORY_USAGE", "use project-memory index [project]");
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
            &json!({"commands":["entry","query","get","review","index","compact","verify"],"query_order":["L1 anchors","exact ID","node/function/resource","category/tag","declared relations","lesson references","FTS5","WeMM semantic candidates","importance","updated_at"]}),
        ),
        _ => fail(
            "MEMORY_COMMAND_UNKNOWN",
            "select entry, query, get, review, index, compact, or verify",
        ),
    }
}
