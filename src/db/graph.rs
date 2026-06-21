//! Code-graph persistence and queries (symbols + edges).
//!
//! Edges store the callee/import target by *name*; resolution to a concrete
//! definition happens at query time via a join on `symbols.name`. This keeps
//! incremental reindexing simple (no dangling symbol ids) at the cost of
//! name-level precision — good enough for agent navigation.

use crate::db::Database;
use crate::error::DbResult;
use crate::services::ast::RawCall;
use crate::types::FileId;
use rusqlite::params;
use rusqlite::types::ToSql;
use std::collections::HashSet;

/// A symbol row returned from graph queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSymbol {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// One definition to persist (mirrors [`crate::services::ast::AstSymbol`]).
pub struct SymbolRow {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// One file's extracted graph rows for batch replacement.
pub struct FileGraphBatchItem<'a> {
    pub file_id: FileId,
    pub symbols: &'a [SymbolRow],
    pub calls: &'a [RawCall],
    pub imports: &'a [String],
}

const GRAPH_DELETE_FILE_ID_CHUNK: usize = 900;

impl Database {
    /// Replaces all graph data for `file_id` with the given symbols/edges.
    /// Call edges are attributed to the innermost symbol whose byte range
    /// contains the call site; calls outside any symbol are dropped.
    pub fn replace_file_graph(
        &self,
        file_id: FileId,
        symbols: &[SymbolRow],
        calls: &[RawCall],
        imports: &[String],
    ) -> DbResult<()> {
        let conn = self.conn()?;
        Self::replace_file_graph_on(&conn, file_id, symbols, calls, imports)
    }

    /// Same as [`Self::replace_file_graph`] but on a caller-provided
    /// connection (used during batch indexing with indexing pragmas).
    pub fn replace_file_graph_on(
        conn: &rusqlite::Connection,
        file_id: FileId,
        symbols: &[SymbolRow],
        calls: &[RawCall],
        imports: &[String],
    ) -> DbResult<()> {
        let graph = FileGraphBatchItem {
            file_id,
            symbols,
            calls,
            imports,
        };
        Self::replace_file_graphs_on(conn, std::slice::from_ref(&graph))
    }

    /// Replaces graph data for many files in one transaction.
    pub fn replace_file_graphs_on(
        conn: &rusqlite::Connection,
        graphs: &[FileGraphBatchItem<'_>],
    ) -> DbResult<()> {
        if graphs.is_empty() {
            return Ok(());
        }

        super::with_transaction(conn, || Self::replace_file_graphs_no_tx(conn, graphs))
    }

    fn replace_file_graphs_no_tx(
        conn: &rusqlite::Connection,
        graphs: &[FileGraphBatchItem<'_>],
    ) -> DbResult<()> {
        let file_ids: Vec<u32> = graphs.iter().map(|graph| graph.file_id.as_u32()).collect();
        // Clear dependent rows first so this works correctly for both legacy
        // v4 databases without foreign keys and newer schemas with cascades.
        delete_graph_rows_for_file_ids(
            conn,
            "DELETE FROM embeddings WHERE file_id IN ({})",
            &file_ids,
        )?;
        delete_graph_rows_for_file_ids(conn, "DELETE FROM edges WHERE file_id IN ({})", &file_ids)?;
        delete_graph_rows_for_file_ids(
            conn,
            "DELETE FROM symbols WHERE file_id IN ({})",
            &file_ids,
        )?;

        let mut insert_symbol = conn.prepare_cached(
            "INSERT INTO symbols
             (file_id, name, kind, start_line, end_line, start_byte, end_byte)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut insert_edge = conn.prepare_cached(
            "INSERT INTO edges (src_symbol, dst_name, kind, file_id)
             VALUES (?1, ?2, ?3, ?4)",
        )?;

        for graph in graphs {
            let fid = graph.file_id.as_u32();

            // Insert symbols, remembering (symbol_id, start_byte, end_byte) so
            // we can attribute call sites to their enclosing definition.
            let mut ranges: Vec<(i64, usize, usize)> = Vec::with_capacity(graph.symbols.len());
            for s in graph.symbols {
                insert_symbol.execute(params![
                    fid,
                    &s.name,
                    &s.kind,
                    s.start_line as i64,
                    s.end_line as i64,
                    s.start_byte as i64,
                    s.end_byte as i64,
                ])?;
                let id = conn.last_insert_rowid();
                ranges.push((id, s.start_byte, s.end_byte));
            }

            let mut seen_calls: HashSet<(i64, &str)> = HashSet::new();
            for c in graph.calls {
                if let Some(src) = innermost(&ranges, c.byte) {
                    if seen_calls.insert((src, c.name.as_str())) {
                        insert_edge.execute(params![src, &c.name, "CALLS", fid])?;
                    }
                }
            }
            let mut seen_imports: HashSet<&str> = HashSet::new();
            for m in graph.imports {
                if seen_imports.insert(m.as_str()) {
                    insert_edge.execute(params![Option::<i64>::None, m, "IMPORTS", fid])?;
                }
            }
        }

        Ok(())
    }

    /// Removes all graph rows for a file (used when a file is deleted).
    pub fn delete_file_graph(&self, file_id: FileId) -> DbResult<()> {
        let conn = self.conn()?;
        Self::delete_file_graph_on(&conn, file_id)
    }

    /// Stores one symbol embedding on a caller-provided connection.
    pub fn upsert_embedding_on(
        conn: &rusqlite::Connection,
        symbol_id: i64,
        file_id: FileId,
        vec_blob: &[u8],
    ) -> DbResult<()> {
        conn.execute(
            "INSERT INTO embeddings (symbol_id, file_id, vec) VALUES (?1, ?2, ?3)
             ON CONFLICT(symbol_id) DO UPDATE SET vec = excluded.vec, file_id = excluded.file_id",
            params![symbol_id, file_id.as_u32(), vec_blob],
        )?;
        Ok(())
    }

    /// The symbol id range (min, max) for a file — used to map freshly inserted
    /// symbols back to their ids for embedding. Returns None if the file has
    /// no symbols.
    pub fn file_symbol_ids(&self, file_id: FileId) -> DbResult<Vec<(i64, String, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT symbol_id, name, kind FROM symbols WHERE file_id = ?1 ORDER BY symbol_id",
        )?;
        let rows = stmt
            .query_map(params![file_id.as_u32()], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// All stored embeddings as `(symbol_id, file_id, vector)`. Intended for
    /// diagnostics or offline analysis; search reranking should use bounded
    /// embedding queries.
    pub fn all_embeddings(&self) -> DbResult<Vec<(i64, u32, Vec<f32>)>> {
        use crate::services::semantic::decode;
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached("SELECT symbol_id, file_id, vec FROM embeddings")?;
        let rows = stmt
            .query_map([], |r| {
                let blob: Vec<u8> = r.get(2)?;
                Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?, decode(&blob)))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Stored embeddings for a bounded set of files as `(symbol_id, file_id, vector)`.
    pub fn embeddings_for_files(&self, file_ids: &[FileId]) -> DbResult<Vec<(i64, u32, Vec<f32>)>> {
        use crate::services::semantic::decode;
        if file_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn()?;
        let placeholders: Vec<String> = (1..=file_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT symbol_id, file_id, vec FROM embeddings WHERE file_id IN ({})",
            placeholders.join(",")
        );
        let ids: Vec<u32> = file_ids.iter().map(|id| id.as_u32()).collect();
        let params: Vec<&dyn ToSql> = ids.iter().map(|id| id as &dyn ToSql).collect();

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params.as_slice(), |r| {
                let blob: Vec<u8> = r.get(2)?;
                Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?, decode(&blob)))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Graph-row cleanup on a caller-provided connection.
    pub fn delete_file_graph_on(conn: &rusqlite::Connection, file_id: FileId) -> DbResult<()> {
        let fid = file_id.as_u32();
        for sql in [
            "DELETE FROM embeddings WHERE file_id = ?1",
            "DELETE FROM edges WHERE file_id = ?1",
            "DELETE FROM symbols WHERE file_id = ?1",
        ] {
            conn.execute(sql, params![fid])?;
        }
        Ok(())
    }

    /// Definitions named `name`.
    pub fn symbol_defs(&self, name: &str) -> DbResult<Vec<GraphSymbol>> {
        self.query_symbols(
            "SELECT s.name, s.kind, f.path, s.start_line, s.end_line
             FROM symbols s JOIN files f ON f.file_id = s.file_id
             WHERE s.name = ?1
             ORDER BY f.path, s.start_line",
            params![name],
        )
    }

    /// Symbols that call `name` (direct callers).
    pub fn callers(&self, name: &str) -> DbResult<Vec<GraphSymbol>> {
        self.callers_limited(name, usize::MAX)
    }

    /// Symbols that call `name`, capped to `limit` rows.
    pub fn callers_limited(&self, name: &str, limit: usize) -> DbResult<Vec<GraphSymbol>> {
        self.query_symbols(
            "SELECT DISTINCT s.name, s.kind, f.path, s.start_line, s.end_line
             FROM edges e
             JOIN symbols s ON s.symbol_id = e.src_symbol
             JOIN files f ON f.file_id = s.file_id
             WHERE e.kind = 'CALLS' AND e.dst_name = ?1
             ORDER BY f.path, s.start_line
             LIMIT ?2",
            params![name, limit_param(limit)],
        )
    }

    /// Names called by any definition named `name`, resolved to definitions
    /// when they exist in the index. Returns `(callee_name, Option<def>)`.
    pub fn callees(&self, name: &str) -> DbResult<Vec<(String, Option<GraphSymbol>)>> {
        self.query_callees_unbounded(name)
    }

    /// Names called by `name`, resolved to at most one definition each, capped
    /// to `limit` rows.
    pub fn callees_limited(
        &self,
        name: &str,
        limit: usize,
    ) -> DbResult<Vec<(String, Option<GraphSymbol>)>> {
        if limit == usize::MAX {
            return self.query_callees_unbounded(name);
        }

        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(
            "WITH names AS (
                SELECT DISTINCT e.dst_name
                FROM edges e
             JOIN symbols src ON src.symbol_id = e.src_symbol
             WHERE e.kind = 'CALLS' AND src.name = ?1
             ORDER BY e.dst_name
             LIMIT ?2
             ),
             resolved AS (
                SELECT
                    names.dst_name AS callee_name,
                    s.name AS def_name,
                    s.kind,
                    f.path,
                    s.start_line,
                    s.end_line,
                    ROW_NUMBER() OVER (
                        PARTITION BY names.dst_name
                        ORDER BY f.path, s.start_line
                    ) AS rn
                FROM names
                LEFT JOIN symbols s ON s.name = names.dst_name
                LEFT JOIN files f ON f.file_id = s.file_id
             )
             SELECT callee_name, def_name, kind, path, start_line, end_line
             FROM resolved
             WHERE rn = 1
             ORDER BY callee_name",
        )?;

        let rows = stmt
            .query_map(params![name, limit_param(limit)], |r| {
                let callee: String = r.get(0)?;
                let def_name: Option<String> = r.get(1)?;
                let def = match def_name {
                    Some(name) => Some(GraphSymbol {
                        name,
                        kind: r.get(2)?,
                        path: r.get(3)?,
                        start_line: r.get::<_, i64>(4)? as usize,
                        end_line: r.get::<_, i64>(5)? as usize,
                    }),
                    None => None,
                };
                Ok((callee, def))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn query_callees_unbounded(&self, name: &str) -> DbResult<Vec<(String, Option<GraphSymbol>)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(
            "WITH names AS (
                SELECT DISTINCT e.dst_name
                FROM edges e
                JOIN symbols src ON src.symbol_id = e.src_symbol
                WHERE e.kind = 'CALLS' AND src.name = ?1
             ),
             resolved AS (
                SELECT
                    names.dst_name AS callee_name,
                    s.name AS def_name,
                    s.kind,
                    f.path,
                    s.start_line,
                    s.end_line,
                    ROW_NUMBER() OVER (
                        PARTITION BY names.dst_name
                        ORDER BY f.path, s.start_line
                    ) AS rn
                FROM names
                LEFT JOIN symbols s ON s.name = names.dst_name
                LEFT JOIN files f ON f.file_id = s.file_id
             )
             SELECT callee_name, def_name, kind, path, start_line, end_line
             FROM resolved
             WHERE rn = 1
             ORDER BY callee_name",
        )?;

        let rows = stmt
            .query_map(params![name], |r| {
                let callee: String = r.get(0)?;
                let def_name: Option<String> = r.get(1)?;
                let def = match def_name {
                    Some(name) => Some(GraphSymbol {
                        name,
                        kind: r.get(2)?,
                        path: r.get(3)?,
                        start_line: r.get::<_, i64>(4)? as usize,
                        end_line: r.get::<_, i64>(5)? as usize,
                    }),
                    None => None,
                };
                Ok((callee, def))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Transitive callees of `name` up to `max_depth`, cycle-safe via UNION.
    pub fn call_chain(&self, name: &str, max_depth: usize) -> DbResult<Vec<GraphSymbol>> {
        self.call_chain_limited(name, max_depth, usize::MAX)
    }

    /// Transitive callees of `name` up to `max_depth`, capped to `limit` rows.
    pub fn call_chain_limited(
        &self,
        name: &str,
        max_depth: usize,
        limit: usize,
    ) -> DbResult<Vec<GraphSymbol>> {
        self.query_symbols(
            "WITH RECURSIVE chain(sym, depth) AS (
                SELECT symbol_id, 0 FROM symbols WHERE name = ?1
                UNION
                SELECT s2.symbol_id, chain.depth + 1
                FROM chain
                JOIN edges e ON e.src_symbol = chain.sym AND e.kind = 'CALLS'
                JOIN symbols s2 ON s2.name = e.dst_name
                WHERE chain.depth < ?2
             )
             SELECT DISTINCT s.name, s.kind, f.path, s.start_line, s.end_line
             FROM chain
             JOIN symbols s ON s.symbol_id = chain.sym
             JOIN files f ON f.file_id = s.file_id
             WHERE chain.depth > 0
             ORDER BY f.path, s.start_line
             LIMIT ?3",
            params![name, max_depth as i64, limit_param(limit)],
        )
    }

    /// Modules imported by the file at `path`.
    pub fn imports_of(&self, path: &str) -> DbResult<Vec<String>> {
        self.imports_of_limited(path, usize::MAX)
    }

    /// Modules imported by the file at `path`, capped to `limit` rows.
    pub fn imports_of_limited(&self, path: &str, limit: usize) -> DbResult<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT e.dst_name
             FROM edges e JOIN files f ON f.file_id = e.file_id
             WHERE e.kind = 'IMPORTS' AND f.path = ?1
             ORDER BY e.dst_name
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![path, limit_param(limit)], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// File paths that import a module whose name contains `module`.
    pub fn dependents_of(&self, module: &str) -> DbResult<Vec<String>> {
        self.dependents_of_limited(module, usize::MAX)
    }

    /// File paths that import a module whose name contains `module`, capped to
    /// `limit` rows.
    pub fn dependents_of_limited(&self, module: &str, limit: usize) -> DbResult<Vec<String>> {
        let conn = self.conn()?;
        let like = format!("%{}%", escape_like(module));
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT f.path
             FROM edges e JOIN files f ON f.file_id = e.file_id
             WHERE e.kind = 'IMPORTS' AND e.dst_name LIKE ?1 ESCAPE '\\'
             ORDER BY f.path
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![like, limit_param(limit)], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn query_symbols(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> DbResult<Vec<GraphSymbol>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(sql)?;
        let rows = stmt
            .query_map(params, |r| {
                Ok(GraphSymbol {
                    name: r.get(0)?,
                    kind: r.get(1)?,
                    path: r.get(2)?,
                    start_line: r.get::<_, i64>(3)? as usize,
                    end_line: r.get::<_, i64>(4)? as usize,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn delete_graph_rows_for_file_ids(
    conn: &rusqlite::Connection,
    sql_template: &str,
    file_ids: &[u32],
) -> DbResult<()> {
    for chunk in file_ids.chunks(GRAPH_DELETE_FILE_ID_CHUNK) {
        let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
        let sql = sql_template.replace("{}", &placeholders.join(","));
        let mut stmt = conn.prepare_cached(&sql)?;
        stmt.execute(rusqlite::params_from_iter(chunk.iter()))?;
    }
    Ok(())
}

/// Innermost (smallest) symbol range containing `byte`. Returns its symbol id.
fn innermost(ranges: &[(i64, usize, usize)], byte: usize) -> Option<i64> {
    ranges
        .iter()
        .filter(|(_, s, e)| *s <= byte && byte < *e)
        .min_by_key(|(_, s, e)| e - s)
        .map(|(id, _, _)| *id)
}

fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn limit_param(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Database {
        Database::in_memory().unwrap()
    }

    fn graph_row_counts(db: &Database) -> (i64, i64, i64) {
        let conn = db.conn().unwrap();
        let symbols = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        let edges = conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        let embeddings = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        (symbols, edges, embeddings)
    }

    #[test]
    fn graph_roundtrip_and_queries() {
        let db = db();
        // Two files: a.rs defines `caller` which calls `target`; b.rs defines target.
        let fa = db
            .upsert_file("a.rs", "fn caller() { target(); }", 1)
            .unwrap();
        let fb = db.upsert_file("b.rs", "fn target() {}", 2).unwrap();

        db.replace_file_graph(
            fa,
            &[SymbolRow {
                name: "caller".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 25,
            }],
            &[RawCall {
                name: "target".into(),
                byte: 14,
            }],
            &["std::io".into()],
        )
        .unwrap();
        db.replace_file_graph(
            fb,
            &[SymbolRow {
                name: "target".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 14,
            }],
            &[],
            &[],
        )
        .unwrap();

        let callers = db.callers("target").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "caller");

        let callees = db.callees("caller").unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].0, "target");
        assert!(callees[0].1.is_some(), "target resolves to a definition");

        let chain = db.call_chain("caller", 5).unwrap();
        assert!(chain.iter().any(|s| s.name == "target"));

        assert_eq!(db.imports_of("a.rs").unwrap(), vec!["std::io".to_string()]);
        assert_eq!(
            db.dependents_of("std::io").unwrap(),
            vec!["a.rs".to_string()]
        );
        assert!(
            db.dependents_of("%").unwrap().is_empty(),
            "LIKE wildcards in user input should be treated literally"
        );
    }

    #[test]
    fn callees_resolve_first_definition_and_keep_unresolved() {
        let db = db();
        let caller = db
            .upsert_file("caller.rs", "fn caller() { target(); missing(); }", 1)
            .unwrap();
        let target_z = db.upsert_file("z_target.rs", "fn target() {}", 2).unwrap();
        let target_a = db.upsert_file("a_target.rs", "fn target() {}", 3).unwrap();

        db.replace_file_graph(
            caller,
            &[SymbolRow {
                name: "caller".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 36,
            }],
            &[
                RawCall {
                    name: "target".into(),
                    byte: 14,
                },
                RawCall {
                    name: "missing".into(),
                    byte: 24,
                },
            ],
            &[],
        )
        .unwrap();
        for file_id in [target_z, target_a] {
            db.replace_file_graph(
                file_id,
                &[SymbolRow {
                    name: "target".into(),
                    kind: "fn".into(),
                    start_line: 1,
                    end_line: 1,
                    start_byte: 0,
                    end_byte: 14,
                }],
                &[],
                &[],
            )
            .unwrap();
        }

        let callees = db.callees("caller").unwrap();
        assert_eq!(callees.len(), 2);
        assert_eq!(callees[0].0, "missing");
        assert!(callees[0].1.is_none(), "unresolved callee should remain");
        assert_eq!(callees[1].0, "target");
        assert_eq!(
            callees[1].1.as_ref().map(|s| s.path.as_str()),
            Some("a_target.rs"),
            "first definition should preserve symbol_defs path/start_line ordering"
        );

        let limited = db.callees_limited("caller", 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].0, "missing");
    }

    #[test]
    fn reindex_replaces_edges() {
        let db = db();
        let f = db.upsert_file("a.rs", "x", 1).unwrap();
        let sym = SymbolRow {
            name: "a".into(),
            kind: "fn".into(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 10,
        };
        db.replace_file_graph(
            f,
            &[sym],
            &[RawCall {
                name: "old".into(),
                byte: 1,
            }],
            &[],
        )
        .unwrap();
        let sym2 = SymbolRow {
            name: "a".into(),
            kind: "fn".into(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 10,
        };
        db.replace_file_graph(
            f,
            &[sym2],
            &[RawCall {
                name: "new".into(),
                byte: 1,
            }],
            &[],
        )
        .unwrap();
        assert!(db.callers("old").unwrap().is_empty(), "old edge gone");
        assert_eq!(db.callers("new").unwrap().len(), 1);
    }

    #[test]
    fn batch_replaces_graphs_for_multiple_files() {
        let db = db();
        let fa = db
            .upsert_file("a.rs", "fn caller() { target(); }", 1)
            .unwrap();
        let fb = db.upsert_file("b.rs", "fn target() {}", 2).unwrap();

        let caller_symbols = [SymbolRow {
            name: "caller".into(),
            kind: "fn".into(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 25,
        }];
        let caller_calls = [RawCall {
            name: "target".into(),
            byte: 14,
        }];
        let caller_imports = vec!["std::fs".to_string()];
        let target_symbols = [SymbolRow {
            name: "target".into(),
            kind: "fn".into(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 14,
        }];

        {
            let conn = db.conn().unwrap();
            Database::replace_file_graphs_on(
                &conn,
                &[
                    FileGraphBatchItem {
                        file_id: fa,
                        symbols: &caller_symbols,
                        calls: &caller_calls,
                        imports: &caller_imports,
                    },
                    FileGraphBatchItem {
                        file_id: fb,
                        symbols: &target_symbols,
                        calls: &[],
                        imports: &[],
                    },
                ],
            )
            .unwrap();
        }

        assert_eq!(db.callers("target").unwrap()[0].name, "caller");
        assert_eq!(db.imports_of("a.rs").unwrap(), vec!["std::fs".to_string()]);
        assert_eq!(graph_row_counts(&db), (2, 2, 0));
    }

    #[test]
    fn repeated_graph_edges_are_stored_once_per_file() {
        let db = db();
        let f = db
            .upsert_file("a.rs", "fn caller() { target(); target(); }", 1)
            .unwrap();
        db.replace_file_graph(
            f,
            &[SymbolRow {
                name: "caller".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 35,
            }],
            &[
                RawCall {
                    name: "target".into(),
                    byte: 14,
                },
                RawCall {
                    name: "target".into(),
                    byte: 24,
                },
            ],
            &["std::io".into(), "std::io".into()],
        )
        .unwrap();

        assert_eq!(
            graph_row_counts(&db),
            (1, 2, 0),
            "one CALLS edge and one IMPORTS edge should be stored"
        );
        assert_eq!(db.callers("target").unwrap().len(), 1);
        assert_eq!(db.imports_of("a.rs").unwrap(), vec!["std::io".to_string()]);
    }

    #[test]
    fn reindex_clears_stale_embeddings() {
        let db = db();
        let f = db.upsert_file("a.rs", "fn a() {}", 1).unwrap();
        db.replace_file_graph(
            f,
            &[SymbolRow {
                name: "a".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 9,
            }],
            &[],
            &[],
        )
        .unwrap();

        let symbol_id = db.file_symbol_ids(f).unwrap()[0].0;
        {
            let conn = db.conn().unwrap();
            Database::upsert_embedding_on(&conn, symbol_id, f, &[0, 0, 0, 0]).unwrap();
        }
        assert_eq!(db.embeddings_for_files(&[f]).unwrap().len(), 1);

        db.replace_file_graph(
            f,
            &[SymbolRow {
                name: "renamed".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 15,
            }],
            &[],
            &[],
        )
        .unwrap();

        assert!(
            db.embeddings_for_files(&[f]).unwrap().is_empty(),
            "old embedding must not survive graph replacement"
        );
    }

    #[test]
    fn deleting_file_cascades_graph_rows() {
        let db = db();
        let f = db
            .upsert_file("a.rs", "fn caller() { target(); }", 1)
            .unwrap();
        db.replace_file_graph(
            f,
            &[SymbolRow {
                name: "caller".into(),
                kind: "fn".into(),
                start_line: 1,
                end_line: 1,
                start_byte: 0,
                end_byte: 25,
            }],
            &[RawCall {
                name: "target".into(),
                byte: 14,
            }],
            &["std::io".into()],
        )
        .unwrap();

        let symbol_id = db.file_symbol_ids(f).unwrap()[0].0;
        {
            let conn = db.conn().unwrap();
            Database::upsert_embedding_on(&conn, symbol_id, f, &[0, 0, 0, 0]).unwrap();
        }
        assert_eq!(graph_row_counts(&db), (1, 2, 1));

        assert!(db.delete_file("a.rs").unwrap());
        assert_eq!(
            graph_row_counts(&db),
            (0, 0, 0),
            "graph data should be owned by the deleted file"
        );
    }
}
