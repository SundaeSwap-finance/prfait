use std::process::Command;

fn main() {
    let output = Command::new("git")
        .args(["show", "origin/process-orders:frontend/sdk/src/sdk/v0_4/index.ts"])
        .current_dir("/home/pi/proj/realfi/realfi")
        .output()
        .unwrap();
    let source = String::from_utf8_lossy(&output.stdout).to_string();

    let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&source, None).unwrap();

    for target in &[
        "updateTreasuryOutput",
        "buildBurnExecute",
        "buildDepositExecute",
        "buildStakeExecute",
        "buildExecuteOrdersTx",
    ] {
        println!("=== Searching for: {target} ===");
        find_and_dump(tree.root_node(), &source, target, 0);
        println!();
    }
}

fn find_and_dump(node: tree_sitter::Node, source: &str, target: &str, depth: usize) {
    // Check various field names that might hold the entity name
    for field_name in &["name", "property", "left", "key"] {
        if let Some(name_node) = node.child_by_field_name(field_name) {
            let name = &source[name_node.byte_range()];
            if name == target {
                let indent = "  ".repeat(depth);
                println!(
                    "{indent}FOUND via field '{field_name}': kind={} at line {}",
                    node.kind(),
                    node.start_position().row + 1
                );

                // Dump all children with field info
                let mut cursor = node.walk();
                for (ci, child) in node.children(&mut cursor).enumerate() {
                    let field = node.field_name_for_child(ci as u32).unwrap_or("(none)");
                    let named = if child.is_named() { "named" } else { "anon" };
                    let preview: String = source[child.byte_range()].chars().take(60).collect();
                    println!(
                        "{indent}  child[{ci}]: kind={} field={field} {named} preview={:?}",
                        child.kind(),
                        preview
                    );
                }

                // Check for body
                if let Some(body) = node.child_by_field_name("body") {
                    println!("{indent}  HAS BODY: kind={}", body.kind());
                } else {
                    println!("{indent}  NO BODY FIELD");
                }
                return;
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        find_and_dump(child, source, target, depth + 1);
    }
}
