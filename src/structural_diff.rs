use similar::{Algorithm, DiffOp, capture_diff_slices};
use tree_sitter::{Language, Node, Parser, Tree};

/// A block in a structural diff
pub enum Block {
    /// Unchanged statements — collapse if many
    Unchanged(Vec<String>),
    /// Removed block of statements
    Removed(Vec<String>),
    /// Added block of statements
    Added(Vec<String>),
    /// Modified: old statements replaced by new statements
    Modified(Vec<String>, Vec<String>),
}

/// Produce a structural diff by parsing full file content with tree-sitter,
/// finding the target entity by name, and diffing its body at the statement level.
///
/// `before_file` / `after_file`: complete file source at base and head refs
/// `entity_name`: name of the entity to diff within the file
/// `file_path`: used to determine language from extension
pub fn structural_diff(
    before_file: &str,
    after_file: &str,
    entity_name: &str,
    file_path: &str,
) -> Option<Vec<Block>> {
    let ext = file_path.rsplit('.').next()?;
    let lang = language_for_ext(ext)?;

    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;

    let before_tree = parser.parse(before_file, None)?;
    let after_tree = parser.parse(after_file, None)?;

    let before_stmts = find_entity_statements(before_tree.root_node(), before_file, entity_name)?;
    let after_stmts = find_entity_statements(after_tree.root_node(), after_file, entity_name)?;

    if before_stmts.is_empty() && after_stmts.is_empty() {
        return None;
    }

    Some(diff_blocks(&before_stmts, &after_stmts))
}

/// Like `structural_diff`, but accepts pre-parsed tree-sitter trees to avoid
/// re-parsing the same files for every entity.
pub fn structural_diff_with_trees(
    before_file: &str,
    after_file: &str,
    before_tree: &Tree,
    after_tree: &Tree,
    entity_name: &str,
) -> Option<Vec<Block>> {
    let before_stmts = find_entity_statements(before_tree.root_node(), before_file, entity_name)?;
    let after_stmts = find_entity_statements(after_tree.root_node(), after_file, entity_name)?;

    if before_stmts.is_empty() && after_stmts.is_empty() {
        return None;
    }

    Some(diff_blocks(&before_stmts, &after_stmts))
}

/// Parse a file with tree-sitter given a file extension. Returns None if the
/// language isn't supported or parsing fails.
pub fn parse_file(file_content: &str, file_path: &str) -> Option<Tree> {
    let ext = file_path.rsplit('.').next()?;
    let lang = language_for_ext(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    parser.parse(file_content, None)
}

/// Recursively search the AST for an entity with the given name,
/// then extract the statements from its body.
fn find_entity_statements(root: Node, source: &str, entity_name: &str) -> Option<Vec<String>> {
    let entity_node = find_named_entity_with_body(root, source, entity_name)?;

    // Get the body of this entity (statement_block, block, etc.)
    let body = find_body_child(entity_node)?;

    collect_children(body, source)
}

/// Recursively walk the AST to find a **definition** node whose name matches entity_name.
/// Only returns nodes that have a body (i.e., actual definitions, not call sites or references).
/// This avoids matching `this.foo(...)` (member_expression with "property" field) when we
/// really want the `method_definition` for `foo`.
fn find_named_entity_with_body<'a>(
    node: Node<'a>,
    source: &str,
    entity_name: &str,
) -> Option<Node<'a>> {
    // Check if this node has a matching name AND a body
    if matches_name(node, source, entity_name) && find_body_child(node).is_some() {
        return Some(node);
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_named_entity_with_body(child, source, entity_name) {
            return Some(found);
        }
    }

    None
}

/// Check whether a node's name or property field matches the given entity name.
fn matches_name(node: Node, source: &str, entity_name: &str) -> bool {
    for field in &["name", "property"] {
        if let Some(name_node) = node.child_by_field_name(field) {
            if &source[name_node.byte_range()] == entity_name {
                return true;
            }
        }
    }
    false
}

/// Find the body/block child of an entity node.
fn find_body_child(node: Node) -> Option<Node> {
    // Try field name "body" first (most languages use this)
    if let Some(body) = node.child_by_field_name("body") {
        return Some(body);
    }

    // Fall back to searching for block-type children
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        if kind == "statement_block"
            || kind == "block"
            || kind == "compound_statement"
            || kind == "function_body"
            || kind.ends_with("_body")
            || kind.ends_with("_block")
        {
            return Some(child);
        }
    }

    None
}

/// Collect named children from a body node, grouping comments with following statements.
fn collect_children(body: Node, source: &str) -> Option<Vec<String>> {
    let mut stmts = Vec::new();
    let mut cursor = body.walk();
    let mut pending_comments = String::new();

    for child in body.children(&mut cursor) {
        let kind = child.kind();

        let is_comment = kind == "comment" || kind == "line_comment" || kind == "block_comment";
        if !child.is_named() && !is_comment {
            continue;
        }

        let text = &source[child.byte_range()];

        if is_comment {
            if !pending_comments.is_empty() {
                pending_comments.push('\n');
            }
            pending_comments.push_str(text);
        } else {
            let full = if pending_comments.is_empty() {
                text.to_string()
            } else {
                let combined = format!("{}\n{}", pending_comments, text);
                pending_comments.clear();
                combined
            };
            stmts.push(full);
        }
    }

    if !pending_comments.is_empty() {
        stmts.push(pending_comments);
    }

    if stmts.is_empty() {
        None
    } else {
        Some(stmts)
    }
}

/// Get a tree-sitter Language for a file extension
fn language_for_ext(ext: &str) -> Option<Language> {
    match ext {
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" | "pyi" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "rb" => Some(tree_sitter_ruby::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "sh" | "bash" => Some(tree_sitter_bash::LANGUAGE.into()),
        _ => None,
    }
}

/// Normalize a statement for comparison: trim each line, collapse whitespace.
fn normalize(s: &str) -> String {
    s.lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n")
}

/// Diff two lists of statements using LCS-based algorithm, producing structural blocks.
fn diff_blocks(before: &[String], after: &[String]) -> Vec<Block> {
    let before_norm: Vec<String> = before.iter().map(|s| normalize(s)).collect();
    let after_norm: Vec<String> = after.iter().map(|s| normalize(s)).collect();
    let ops = capture_diff_slices(Algorithm::Patience, &before_norm, &after_norm);

    let mut blocks = Vec::new();

    for op in ops {
        match op {
            DiffOp::Equal {
                old_index, len, ..
            } => {
                let stmts: Vec<String> =
                    before[old_index..old_index + len].iter().cloned().collect();
                blocks.push(Block::Unchanged(stmts));
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                let stmts: Vec<String> =
                    before[old_index..old_index + old_len].iter().cloned().collect();
                blocks.push(Block::Removed(stmts));
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                let stmts: Vec<String> =
                    after[new_index..new_index + new_len].iter().cloned().collect();
                blocks.push(Block::Added(stmts));
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let old: Vec<String> =
                    before[old_index..old_index + old_len].iter().cloned().collect();
                let new: Vec<String> =
                    after[new_index..new_index + new_len].iter().cloned().collect();
                blocks.push(Block::Modified(old, new));
            }
        }
    }

    blocks
}
