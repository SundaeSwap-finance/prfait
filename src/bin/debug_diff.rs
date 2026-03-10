use std::process::Command;

fn main() {
    // Adjust these to match a real PR's refs
    let repo_path = std::path::Path::new("/home/pi/proj/realfi/realfi");
    let file_path = "frontend/sdk/src/sdk/v0_4/index.ts";

    // Get branches to find a PR
    let branches = Command::new("git")
        .args(["branch", "-r", "--list", "origin/*"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    let branches_str = String::from_utf8_lossy(&branches.stdout);
    println!("Remote branches (first 20):");
    for line in branches_str.lines().take(20) {
        println!("  {}", line.trim());
    }

    // Find the merge base between main and a feature branch
    // First, let's see what the PR helper would use
    let base_ref = "main";

    // Find branches that touch our file
    let log = Command::new("git")
        .args([
            "log",
            "--all",
            "--oneline",
            "-10",
            "--",
            file_path,
        ])
        .current_dir(repo_path)
        .output()
        .unwrap();
    println!("\nRecent commits touching {}:", file_path);
    println!("{}", String::from_utf8_lossy(&log.stdout));

    // Try to get the file at HEAD and HEAD~1 to test parsing
    let head_content = Command::new("git")
        .args(["show", &format!("HEAD:{file_path}")])
        .current_dir(repo_path)
        .output()
        .unwrap();

    if !head_content.status.success() {
        println!("Failed to get file at HEAD");
        return;
    }

    let head_src = String::from_utf8_lossy(&head_content.stdout);
    println!("\nFile at HEAD: {} bytes, {} lines", head_src.len(), head_src.lines().count());

    // Parse with tree-sitter
    let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).unwrap();

    let tree = parser.parse(head_src.as_ref(), None).unwrap();
    let root = tree.root_node();
    println!("Parse errors: {}", root.has_error());
    println!("Root kind: {}, named children: {}", root.kind(), root.named_child_count());

    // Search for buildExecuteOrdersTx
    let entity_name = "buildExecuteOrdersTx";
    println!("\nSearching for entity: {}", entity_name);

    if let Some(node) = find_named_entity(root, &head_src, entity_name) {
        println!("Found! kind={}, lines {}-{}", node.kind(), node.start_position().row + 1, node.end_position().row + 1);

        // Find body
        if let Some(body) = node.child_by_field_name("body") {
            println!("Body kind={}, named children: {}", body.kind(), body.named_child_count());

            let mut cursor = body.walk();
            let mut count = 0;
            for child in body.named_children(&mut cursor) {
                count += 1;
                let text = &head_src[child.byte_range()];
                let preview: String = text.chars().take(80).collect();
                println!("  stmt {}: kind={} preview={:?}", count, child.kind(), preview);
            }
            println!("Total statements: {}", count);
        } else {
            println!("No 'body' field found");
            // Try other approaches
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                println!("  child: kind={}", child.kind());
            }
        }
    } else {
        println!("Entity not found! Dumping top-level structure:");
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            let kind = child.kind();
            // Look for classes
            if kind == "class_declaration" || kind == "export_statement" {
                println!("  {} at line {}", kind, child.start_position().row + 1);
                // Check class name
                if let Some(name) = child.child_by_field_name("name") {
                    println!("    name: {}", &head_src[name.byte_range()]);
                }
                // Check for nested exports
                let mut cursor2 = child.walk();
                for grandchild in child.named_children(&mut cursor2) {
                    if let Some(name) = grandchild.child_by_field_name("name") {
                        println!("    nested {} name: {}", grandchild.kind(), &head_src[name.byte_range()]);
                    }
                }
            }
        }
    }
}

fn find_named_entity<'a>(node: tree_sitter::Node<'a>, source: &str, entity_name: &str) -> Option<tree_sitter::Node<'a>> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = &source[name_node.byte_range()];
        if name == entity_name {
            return Some(node);
        }
    }
    if let Some(name_node) = node.child_by_field_name("property") {
        let name = &source[name_node.byte_range()];
        if name == entity_name {
            return Some(node);
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_named_entity(child, source, entity_name) {
            return Some(found);
        }
    }
    None
}
