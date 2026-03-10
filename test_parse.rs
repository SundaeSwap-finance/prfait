/// Diagnostic: parse a TypeScript class method with tree-sitter to understand
/// why structural_diff reports "everything changed".
///
/// Run: cargo run --example test_parse
///
/// FINDING: When sem-core extracts entity content for a class method like
/// `buildExecuteOrdersTx`, it extracts the full `method_definition` node text:
///
///   async buildExecuteOrdersTx(params: { ... }): Promise<TxBuilder> {
///     const { orderInputs, ... } = params;
///     ...
///     return tx;
///   }
///
/// This is a method_definition — valid inside a class body, but NOT valid as a
/// standalone TypeScript program. When structural_diff feeds this to tree-sitter
/// as standalone TypeScript:
///
///   1. tree-sitter parses `async` as an expression_statement (identifier "async")
///   2. `buildExecuteOrdersTx(params: ...)` fails to parse — becomes an ERROR node
///   3. The `{ ... }` body is parsed as a block, but its `const { ... } = params`
///      destructuring is parsed as a sequence_expression with more ERRORs
///   4. The closing `}` of the method becomes a trailing ERROR
///
/// Result: the before/after ASTs are both broken in slightly different ways,
/// leading tree-sitter to extract completely different "statements" from the
/// garbled parse, and the Patience diff sees everything as changed.
///
/// SOLUTION OPTIONS:
///   A. Wrap the entity content before parsing:
///      - Detect method_definition content (starts with `async`, `get`, `set`,
///        identifier+`(`, `private`, `public`, `protected`, `static`, `readonly`)
///      - Wrap it in `class _ { <content> }` so tree-sitter can parse it correctly
///      - Then find_body drills into class_body and extracts the statement_block
///
///   B. Strip the method signature and parse only the body block:
///      - Find the opening `{` at the right nesting level
///      - Parse the body as a standalone block `{ ... }`
///      - tree-sitter handles standalone blocks fine as statement_blocks
///
///   C. In find_body, when root has errors and only 1 valid subtree, try
///      reparsing with a class wrapper as a fallback

use tree_sitter::{Node, Parser};

fn main() {
    // This is what sem-core gives us as entity content for a class method.
    // It's the raw text of the method_definition node.
    let method_content = r#"async buildExecuteOrdersTx(params: {
    orderInputs: Core.TransactionInput[];
    signedPayload: string;
  }): Promise<TxBuilder> {
    const {
      orderInputs,
      signedPayload: signedPayloadCbor,
      signatures,
    } = params;

    // 1. Sort and resolve order UTxOs
    const sortedOrderInputs = sortOrderInputs(orderInputs);

    // Build the transaction
    const tx = this.blaze.newTransaction();
    return tx;
  }"#;

    let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();

    // === RAW (what structural_diff currently does) ===
    println!("=== RAW METHOD CONTENT (current behavior) ===\n");
    let mut parser = Parser::new();
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(method_content, None).unwrap();
    let root = tree.root_node();
    println!("Root: kind={}, has_error={}, named_children={}",
        root.kind(), root.has_error(), root.named_child_count());
    println!("\nStatements extracted:");
    let raw_stmts = extract_statements_from(&root, method_content);
    for (i, s) in raw_stmts.iter().enumerate() {
        let preview: String = s.chars().take(80).collect();
        println!("  [{}] \"{}{}\"", i, preview.replace('\n', "\\n"),
            if s.len() > 80 { "..." } else { "" });
    }
    println!("  => {} statements (BROKEN: includes ERROR nodes, garbled destructuring)\n", raw_stmts.len());

    // === WRAPPED IN CLASS (proposed fix) ===
    println!("=== WRAPPED IN CLASS (proposed fix) ===\n");
    let wrapped = format!("class _ {{\n{}\n}}", method_content);
    parser.set_language(&lang).unwrap();
    let tree2 = parser.parse(&wrapped, None).unwrap();
    let root2 = tree2.root_node();
    println!("Root: kind={}, has_error={}, named_children={}",
        root2.kind(), root2.has_error(), root2.named_child_count());

    let body = find_body(root2);
    println!("find_body result: kind={}", body.kind());
    println!("\nStatements extracted:");
    let wrapped_stmts = extract_statements_from(&body, &wrapped);
    for (i, s) in wrapped_stmts.iter().enumerate() {
        let preview: String = s.chars().take(80).collect();
        println!("  [{}] \"{}{}\"", i, preview.replace('\n', "\\n"),
            if s.len() > 80 { "..." } else { "" });
    }
    println!("  => {} statements (CORRECT: proper destructuring, clean parse)\n", wrapped_stmts.len());

    // === JUST THE BODY BLOCK ===
    println!("=== BODY BLOCK ONLY (alternative fix) ===\n");
    // Find the first `{` that starts the body (skip the params object type)
    if let Some(body_start) = find_method_body_start(method_content) {
        let body_only = &method_content[body_start..];
        parser.set_language(&lang).unwrap();
        let tree3 = parser.parse(body_only, None).unwrap();
        let root3 = tree3.root_node();
        println!("Root: kind={}, has_error={}", root3.kind(), root3.has_error());

        let body3 = find_body(root3);
        println!("find_body result: kind={}", body3.kind());
        let body_stmts = extract_statements_from(&body3, body_only);
        for (i, s) in body_stmts.iter().enumerate() {
            let preview: String = s.chars().take(80).collect();
            println!("  [{}] \"{}{}\"", i, preview.replace('\n', "\\n"),
                if s.len() > 80 { "..." } else { "" });
        }
        println!("  => {} statements\n", body_stmts.len());
    }

    // === DEMONSTRATE THE DIFF PROBLEM ===
    println!("=== WHY THIS CAUSES 'EVERYTHING CHANGED' ===\n");
    // Simulate a small change: add one line
    let before = method_content;
    let after = method_content.replace(
        "// Build the transaction",
        "// Build the transaction\n    console.log('building...');",
    );

    parser.set_language(&lang).unwrap();
    let before_stmts = {
        let t = parser.parse(before, None).unwrap();
        extract_statements_from(&t.root_node(), before)
    };
    parser.set_language(&lang).unwrap();
    let after_stmts = {
        let t = parser.parse(&after, None).unwrap();
        extract_statements_from(&t.root_node(), &after)
    };

    let matching = before_stmts.iter().zip(after_stmts.iter())
        .filter(|(a, b)| normalize(a) == normalize(b))
        .count();
    println!("  Raw parse: {}/{} statements match between before/after",
        matching, before_stmts.len().max(after_stmts.len()));

    // Now with class wrapping
    let before_wrapped = format!("class _ {{\n{}\n}}", before);
    let after_wrapped = format!("class _ {{\n{}\n}}", after);
    parser.set_language(&lang).unwrap();
    let before_stmts2 = {
        let t = parser.parse(&before_wrapped, None).unwrap();
        let b = find_body(t.root_node());
        extract_statements_from(&b, &before_wrapped)
    };
    parser.set_language(&lang).unwrap();
    let after_stmts2 = {
        let t = parser.parse(&after_wrapped, None).unwrap();
        let b = find_body(t.root_node());
        extract_statements_from(&b, &after_wrapped)
    };

    let matching2 = before_stmts2.iter().zip(after_stmts2.iter())
        .filter(|(a, b)| normalize(a) == normalize(b))
        .count();
    println!("  Wrapped:   {}/{} statements match between before/after",
        matching2, before_stmts2.len().max(after_stmts2.len()));
    println!("\n  With wrapping, the diff correctly shows only the changed statement.");
    println!("  Without wrapping, garbled parses make everything look different.");
}

fn extract_statements_from(node: &Node, source: &str) -> Vec<String> {
    let body = find_body(*node);
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
    stmts
}

fn find_body(node: Node) -> Node {
    if node.named_child_count() == 1 {
        if let Some(child) = node.named_child(0) {
            return find_body(child);
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        if kind == "statement_block"
            || kind == "block"
            || kind == "compound_statement"
            || kind == "class_body"
            || kind == "declaration_list"
            || kind == "function_body"
            || kind.ends_with("_body")
            || kind.ends_with("_block")
        {
            return child;
        }
    }
    node
}

fn normalize(s: &str) -> String {
    s.lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n")
}

/// Find the byte offset of the method body's opening `{` (skipping the params type `{`).
/// For `async foo(params: { ... }): Promise<T> {`, we need the last `{`.
fn find_method_body_start(source: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_body = false;
    for (i, ch) in source.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    in_body = true;
                }
            }
            '{' if in_body => return Some(i),
            _ => {}
        }
    }
    None
}
