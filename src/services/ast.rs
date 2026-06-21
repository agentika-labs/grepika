//! Tree-sitter symbol extraction.
//!
//! Replaces the old line-by-line regex heuristics with real AST parsing,
//! giving language-correct symbol kinds and accurate start/end lines for the
//! `outline` tool. Parsing is on-demand (outline is infrequent), so a fresh
//! `Parser` is built per call rather than pooled.

use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

/// A symbol extracted from source via tree-sitter.
pub struct AstSymbol {
    pub name: String,
    pub kind: String,
    /// 1-indexed start line.
    pub line: usize,
    /// 1-indexed end line.
    pub end_line: usize,
    /// Byte range of the whole definition (for future graph/embedding use).
    pub start_byte: usize,
    pub end_byte: usize,
    /// Nesting depth among captured symbols (0 = top level).
    pub level: usize,
}

/// Resolves a file extension to a tree-sitter language + its symbol query.
/// Returns `None` for unsupported languages (caller falls back / returns empty).
fn lang_for(file_type: &str) -> Option<(Language, &'static str)> {
    let (lang, query) = match file_type {
        "rs" => (tree_sitter_rust::LANGUAGE.into(), RUST_QUERY),
        "py" => (tree_sitter_python::LANGUAGE.into(), PYTHON_QUERY),
        "go" => (tree_sitter_go::LANGUAGE.into(), GO_QUERY),
        "js" | "jsx" => (tree_sitter_javascript::LANGUAGE.into(), JS_QUERY),
        "ts" => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), TS_QUERY),
        "tsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), TS_QUERY),
        _ => return None,
    };
    Some((lang, query))
}

/// Whether `outline` can use tree-sitter for this file type.
pub fn is_supported(file_type: &str) -> bool {
    lang_for(file_type).is_some()
}

/// Extracts symbols from `content`. Returns empty if the language is
/// unsupported or the source fails to parse.
pub fn extract(content: &str, file_type: &str) -> Vec<AstSymbol> {
    let Some((lang, query_src)) = lang_for(file_type) else {
        return Vec::new();
    };

    let Some(tree) = parse_tree(content, &lang) else {
        return Vec::new();
    };

    extract_symbols_from_tree(&lang, tree.root_node(), content.as_bytes(), query_src)
}

/// A call site: the callee identifier and its byte offset (used to attribute
/// the call to its enclosing symbol).
pub struct RawCall {
    pub name: String,
    pub byte: usize,
}

/// Complete tree-sitter extraction for a file.
#[derive(Default)]
pub struct AstExtraction {
    pub symbols: Vec<AstSymbol>,
    pub calls: Vec<RawCall>,
    pub imports: Vec<String>,
}

/// Extracts symbols, calls, and imports from `content` with a single parse.
/// Returns an empty extraction for unsupported languages or parse failures.
pub fn extract_all(content: &str, file_type: &str) -> AstExtraction {
    let Some((lang, query_src)) = lang_for(file_type) else {
        return AstExtraction::default();
    };

    let Some(tree) = parse_tree(content, &lang) else {
        return AstExtraction::default();
    };
    let src = content.as_bytes();

    let symbols = extract_symbols_from_tree(&lang, tree.root_node(), src, query_src);
    let (calls, imports) = extract_edges_from_tree(&lang, tree.root_node(), src, file_type);

    AstExtraction {
        symbols,
        calls,
        imports,
    }
}

fn parse_tree(content: &str, lang: &Language) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(lang).ok()?;
    parser.parse(content, None)
}

fn extract_symbols_from_tree(
    lang: &Language,
    root: Node<'_>,
    src: &[u8],
    query_src: &str,
) -> Vec<AstSymbol> {
    let Ok(query) = Query::new(lang, query_src) else {
        return Vec::new();
    };

    let name_idx = query.capture_index_for_name("name");
    let def_idx = query.capture_index_for_name("def");
    let (Some(name_idx), Some(def_idx)) = (name_idx, def_idx) else {
        return Vec::new();
    };

    let mut cursor = QueryCursor::new();
    let mut raw: Vec<AstSymbol> = Vec::new();

    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        let mut name = None;
        let mut def = None;
        for cap in m.captures {
            if cap.index == name_idx {
                name = cap.node.utf8_text(src).ok();
            } else if cap.index == def_idx {
                def = Some(cap.node);
            }
        }
        let (Some(name), Some(def)) = (name, def) else {
            continue;
        };
        raw.push(AstSymbol {
            name: name.to_string(),
            kind: friendly_kind(def.kind()),
            line: def.start_position().row + 1,
            end_line: def.end_position().row + 1,
            start_byte: def.start_byte(),
            end_byte: def.end_byte(),
            level: 0,
        });
    }

    // Nesting level = number of other definitions whose byte range strictly
    // contains this one. O(n²) is acceptable for per-file symbol counts.
    for i in 0..raw.len() {
        let (s, e) = (raw[i].start_byte, raw[i].end_byte);
        raw[i].level = raw
            .iter()
            .filter(|o| {
                o.start_byte < s && o.end_byte >= e && !(o.start_byte == s && o.end_byte == e)
            })
            .count();
    }

    raw.sort_by_key(|s| s.start_byte);
    raw
}

/// Extracts call sites and imported module names from `content`.
/// Returns `(calls, imports)`. Empty for unsupported languages.
pub fn extract_edges(content: &str, file_type: &str) -> (Vec<RawCall>, Vec<String>) {
    let Some((lang, _)) = lang_for(file_type) else {
        return (Vec::new(), Vec::new());
    };
    let Some(tree) = parse_tree(content, &lang) else {
        return (Vec::new(), Vec::new());
    };
    extract_edges_from_tree(&lang, tree.root_node(), content.as_bytes(), file_type)
}

fn extract_edges_from_tree(
    lang: &Language,
    root: Node<'_>,
    src: &[u8],
    file_type: &str,
) -> (Vec<RawCall>, Vec<String>) {
    let Some((call_q, import_q)) = edge_queries(file_type) else {
        return (Vec::new(), Vec::new());
    };

    let calls = extract_calls_from_tree(lang, root, src, call_q);
    let imports = extract_imports_from_tree(lang, root, src, import_q);

    (calls, imports)
}

fn edge_queries(file_type: &str) -> Option<(&'static str, &'static str)> {
    let queries = match file_type {
        "rs" => (RUST_CALL_QUERY, RUST_IMPORT_QUERY),
        "py" => (PY_CALL_QUERY, PY_IMPORT_QUERY),
        "go" => (GO_CALL_QUERY, GO_IMPORT_QUERY),
        "js" | "jsx" => (JS_CALL_QUERY, JS_IMPORT_QUERY),
        "ts" | "tsx" => (JS_CALL_QUERY, JS_IMPORT_QUERY),
        _ => return None,
    };
    Some(queries)
}

fn extract_calls_from_tree(
    lang: &Language,
    root: Node<'_>,
    src: &[u8],
    query_src: &str,
) -> Vec<RawCall> {
    let Ok(query) = Query::new(lang, query_src) else {
        return Vec::new();
    };
    let Some(idx) = query.capture_index_for_name("callee") else {
        return Vec::new();
    };

    let mut calls = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut it = cursor.matches(&query, root, src);
    while let Some(m) = it.next() {
        for cap in m.captures {
            if cap.index == idx {
                if let Ok(name) = cap.node.utf8_text(src) {
                    calls.push(RawCall {
                        name: name.to_string(),
                        byte: cap.node.start_byte(),
                    });
                }
            }
        }
    }
    calls
}

fn extract_imports_from_tree(
    lang: &Language,
    root: Node<'_>,
    src: &[u8],
    query_src: &str,
) -> Vec<String> {
    let Ok(query) = Query::new(lang, query_src) else {
        return Vec::new();
    };
    let Some(idx) = query.capture_index_for_name("mod") else {
        return Vec::new();
    };

    let mut imports = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut it = cursor.matches(&query, root, src);
    while let Some(m) = it.next() {
        for cap in m.captures {
            if cap.index == idx {
                if let Ok(name) = cap.node.utf8_text(src) {
                    imports.push(name.trim_matches(['"', '`', '\'']).to_string());
                }
            }
        }
    }
    imports
}

/// Maps a tree-sitter node kind to a short, stable label for output.
fn friendly_kind(node_kind: &str) -> String {
    let mapped = match node_kind {
        "function_item" | "function_declaration" | "function_definition" => "fn",
        "method_declaration" | "method_definition" => "method",
        "struct_item" => "struct",
        "enum_item" | "enum_declaration" => "enum",
        "union_item" => "union",
        "trait_item" => "trait",
        "impl_item" => "impl",
        "mod_item" => "mod",
        "const_item" => "const",
        "static_item" => "static",
        "type_item" | "type_alias_declaration" | "type_declaration" => "type",
        "macro_definition" => "macro",
        "class_declaration" | "class_definition" => "class",
        "interface_declaration" => "interface",
        "lexical_declaration" | "variable_declaration" => "fn",
        other => return other.trim_end_matches("_declaration").to_string(),
    };
    mapped.to_string()
}

const RUST_QUERY: &str = r"
(function_item name: (identifier) @name) @def
(struct_item name: (type_identifier) @name) @def
(enum_item name: (type_identifier) @name) @def
(union_item name: (type_identifier) @name) @def
(trait_item name: (type_identifier) @name) @def
(mod_item name: (identifier) @name) @def
(const_item name: (identifier) @name) @def
(static_item name: (identifier) @name) @def
(type_item name: (type_identifier) @name) @def
(macro_definition name: (identifier) @name) @def
(impl_item type: (type_identifier) @name) @def
(impl_item type: (generic_type (type_identifier) @name)) @def
";

const PYTHON_QUERY: &str = r"
(function_definition name: (identifier) @name) @def
(class_definition name: (identifier) @name) @def
";

const GO_QUERY: &str = r"
(function_declaration name: (identifier) @name) @def
(method_declaration name: (field_identifier) @name) @def
(type_declaration (type_spec name: (type_identifier) @name)) @def
";

const JS_QUERY: &str = r"
(function_declaration name: (identifier) @name) @def
(class_declaration name: (identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
(variable_declarator
  name: (identifier) @name
  value: [(arrow_function) (function_expression)]) @def
";

const TS_QUERY: &str = r"
(function_declaration name: (identifier) @name) @def
(class_declaration name: (type_identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
(interface_declaration name: (type_identifier) @name) @def
(type_alias_declaration name: (type_identifier) @name) @def
(enum_declaration name: (identifier) @name) @def
(variable_declarator
  name: (identifier) @name
  value: [(arrow_function) (function_expression)]) @def
";

// --- Call queries: capture the callee identifier as @callee ---

const RUST_CALL_QUERY: &str = r"
(call_expression function: (identifier) @callee)
(call_expression function: (scoped_identifier name: (identifier) @callee))
(call_expression function: (field_expression field: (field_identifier) @callee))
(macro_invocation macro: (identifier) @callee)
";

const PY_CALL_QUERY: &str = r"
(call function: (identifier) @callee)
(call function: (attribute attribute: (identifier) @callee))
";

const GO_CALL_QUERY: &str = r"
(call_expression function: (identifier) @callee)
(call_expression function: (selector_expression field: (field_identifier) @callee))
";

const JS_CALL_QUERY: &str = r"
(call_expression function: (identifier) @callee)
(call_expression function: (member_expression property: (property_identifier) @callee))
";

// --- Import queries: capture the module/path as @mod ---

const RUST_IMPORT_QUERY: &str = r"
(use_declaration argument: (_) @mod)
";

const PY_IMPORT_QUERY: &str = r"
(import_statement name: (dotted_name) @mod)
(import_from_statement module_name: (dotted_name) @mod)
";

const GO_IMPORT_QUERY: &str = r"
(import_spec path: (interpreted_string_literal) @mod)
";

const JS_IMPORT_QUERY: &str = r"
(import_statement source: (string (string_fragment) @mod))
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_symbols() {
        let src = "pub fn foo() {}\nstruct Bar { x: u32 }\nimpl Bar { fn baz(&self) {} }\n";
        let syms = extract(src, "rs");
        let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"Bar"));
        assert!(names.contains(&"baz"));
        // baz is nested inside the impl block's struct? It's inside impl_item,
        // which is not captured (no name), so level stays 0. fn foo top-level.
        let foo = syms.iter().find(|s| s.name == "foo").unwrap();
        assert_eq!(foo.kind, "fn");
        assert_eq!(foo.line, 1);
    }

    #[test]
    fn python_nesting() {
        let src = "class A:\n    def m(self):\n        pass\n";
        let syms = extract(src, "py");
        let m = syms.iter().find(|s| s.name == "m").unwrap();
        assert_eq!(m.kind, "fn"); // python function_definition -> fn
        assert_eq!(m.level, 1, "method nested inside class A");
        assert!(syms.iter().any(|s| s.name == "A" && s.kind == "class"));
    }

    #[test]
    fn unsupported_returns_empty() {
        assert!(extract("whatever", "txt").is_empty());
    }

    #[test]
    fn rust_edges() {
        let src = "use foo::bar;\nfn a() { b(); helper(); }\nfn b() {}\n";
        let (calls, imports) = extract_edges(src, "rs");
        let callees: Vec<_> = calls.iter().map(|c| c.name.as_str()).collect();
        assert!(callees.contains(&"b"));
        assert!(callees.contains(&"helper"));
        assert!(imports.iter().any(|i| i.contains("bar")));

        // Attribute the `b()` call to its enclosing fn `a` by byte range.
        let syms = extract(src, "rs");
        let a = syms.iter().find(|s| s.name == "a").unwrap();
        let b_call = calls.iter().find(|c| c.name == "b").unwrap();
        assert!(b_call.byte >= a.start_byte && b_call.byte < a.end_byte);
    }

    #[test]
    fn py_imports() {
        let src = "import os\nfrom collections import deque\n";
        let (_calls, imports) = extract_edges(src, "py");
        assert!(imports.iter().any(|i| i == "os"));
        assert!(imports.iter().any(|i| i == "collections"));
    }

    #[test]
    fn ts_interface() {
        let src = "interface Foo { x: number }\nclass Bar {}\n";
        let syms = extract(src, "ts");
        assert!(syms
            .iter()
            .any(|s| s.name == "Foo" && s.kind == "interface"));
        assert!(syms.iter().any(|s| s.name == "Bar" && s.kind == "class"));
    }
}
