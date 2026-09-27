use super::*;

fn generated_id(value: &Value) -> String {
    let title = value.get("title").and_then(Value::as_str).unwrap_or("");
    let content = entry_text(value);
    let seed = format!("{}\0{}\0{}", title, content, tags(value).join("\0"));
    format!("memory-{}", &digest(&seed)[7..23])
}

fn write_entry_with_review(
    root: &Path,
    global: bool,
    value: Value,
    reviewed: Option<(i64, Vec<String>)>,
) -> Value {
    write_entry_event(root, global, value, reviewed, true)
}

fn write_entry_event(
    root: &Path,
    global: bool,
    mut value: Value,
    reviewed: Option<(i64, Vec<String>)>,
    rebuild: bool,
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
        let index = if rebuild {
            sync_index(root, global)
        } else {
            Value::Null
        };
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
    let index = if rebuild {
        sync_index(root, global)
    } else {
        Value::Null
    };
    json!({"accepted": true, "write_mode": "one_shot", "id": id, "category": cat, "memory_level": memory_level(&value), "review_status": review_status(&value), "detail_path": detail_display_path(global, &id, memory_level(&value)), "index": index})
}

pub(super) fn write_entry(root: &Path, global: bool, value: Value) -> Value {
    write_entry_with_review(root, global, value, None)
}

// Ingest additions before any projection can hide them. Existing IDs remain
// canonical raw history; intentional edits still use the explicit import path.
pub(super) fn import_new_l3(root: &Path, global: bool) -> bool {
    assert_memory_dir(root);
    let existing = effective_entries_for_scope(root, global)
        .iter()
        .map(entry_id)
        .collect::<BTreeSet<_>>();
    let entries = read_selected_details(root, global, true)
        .into_iter()
        .filter(|entry| !existing.contains(&entry_id(entry)))
        .collect::<Vec<_>>();
    // Validate the whole batch before appending or regenerating any detail.
    for entry in &entries {
        assert_id(&entry_id(entry));
        if entry
            .get("category")
            .is_some_and(|value| !value.is_string())
        {
            fail(
                "MEMORY_DETAIL_INVALID",
                "detail metadata category must be a string",
            );
        }
        category(
            entry
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("knowledge"),
        );
    }
    let changed = !entries.is_empty();
    for mut entry in entries {
        entry.as_object_mut().unwrap().remove("review_evidence");
        write_entry_event(root, global, entry, None, false);
    }
    changed
}

fn open_scope(root: &Path, global: bool) -> Connection {
    import_new_l3(root, global);
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
    let collab_home = collab_home_dir();
    let path = collab_home.join("runs").join(run_id).join("notes.jsonl");
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
                "{}/runs/{}/notes.jsonl#{}",
                collab_home_display(),
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

pub(super) fn compact(root: &Path) -> Value {
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
        if import_new_l3(root, global) {
            sync_index(root, global);
        }
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

fn run_notes(_root: &Path, run_id: &str) -> (PathBuf, Vec<Value>) {
    assert_id(run_id);
    let path = collab_home_dir()
        .join("runs")
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
