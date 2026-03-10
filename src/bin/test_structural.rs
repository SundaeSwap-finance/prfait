use std::process::Command;

fn main() {
    let repo_path = std::path::Path::new("/home/pi/proj/realfi/realfi");
    let file_path = "frontend/sdk/src/sdk/v0_4/index.ts";
    let base_branch = "main";
    let head_branch = "process-orders";

    // 1. Compute merge-base (same as app.rs build_file_context)
    let merge_base = Command::new("git")
        .args([
            "merge-base",
            &format!("origin/{base_branch}"),
            &format!("origin/{head_branch}"),
        ])
        .current_dir(repo_path)
        .output()
        .unwrap();

    if !merge_base.status.success() {
        eprintln!("merge-base failed: {}", String::from_utf8_lossy(&merge_base.stderr));
        return;
    }

    let base_ref = String::from_utf8_lossy(&merge_base.stdout).trim().to_string();
    let head_ref = format!("origin/{head_branch}");
    println!("base_ref: {base_ref}");
    println!("head_ref: {head_ref}");

    // 2. git show files at both refs (same as diff_panel.rs git_show_file)
    let before = git_show(repo_path, &base_ref, file_path);
    let after = git_show(repo_path, &head_ref, file_path);

    match (&before, &after) {
        (Some(b), Some(a)) => {
            println!("before: {} bytes, {} lines", b.len(), b.lines().count());
            println!("after: {} bytes, {} lines", a.len(), a.lines().count());
        }
        _ => {
            println!("Failed to get files!");
            println!("before: {:?}", before.as_ref().map(|s| s.len()));
            println!("after: {:?}", after.as_ref().map(|s| s.len()));
            return;
        }
    }

    let before = before.unwrap();
    let after = after.unwrap();

    // 3. Test structural_diff with some entity names
    let test_names = [
        "buildExecuteOrdersTx",
        "updateTreasuryOutput",
        "updateVaultOutput",
        "buildBurnExecute",
        "buildDepositExecute",
        "buildMintExecute",
        "buildWithdrawExecute",
        "buildStakeExecute",
        "RealfiSDKV0_4",
    ];

    for name in &test_names {
        println!("\n--- Testing entity: {name} ---");
        let result = prfait::structural_diff::structural_diff(&before, &after, name, file_path);
        match result {
            Some(blocks) => {
                println!("Got {} blocks!", blocks.len());
                for (i, block) in blocks.iter().enumerate() {
                    match block {
                        prfait::structural_diff::Block::Unchanged(stmts) => {
                            println!("  block {i}: Unchanged ({} stmts)", stmts.len());
                        }
                        prfait::structural_diff::Block::Removed(stmts) => {
                            println!("  block {i}: Removed ({} stmts)", stmts.len());
                            for s in stmts {
                                let preview: String = s.chars().take(80).collect();
                                println!("    - {preview}...");
                            }
                        }
                        prfait::structural_diff::Block::Added(stmts) => {
                            println!("  block {i}: Added ({} stmts)", stmts.len());
                            for s in stmts {
                                let preview: String = s.chars().take(80).collect();
                                println!("    + {preview}...");
                            }
                        }
                        prfait::structural_diff::Block::Modified(old, new) => {
                            println!("  block {i}: Modified ({} -> {} stmts)", old.len(), new.len());
                            for s in old {
                                let preview: String = s.chars().take(80).collect();
                                println!("    - {preview}...");
                            }
                            for s in new {
                                let preview: String = s.chars().take(80).collect();
                                println!("    + {preview}...");
                            }
                        }
                    }
                }
            }
            None => {
                println!("structural_diff returned None!");
            }
        }
    }
}

fn git_show(repo_path: &std::path::Path, git_ref: &str, file_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["show", &format!("{git_ref}:{file_path}")])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        eprintln!("git show {}:{} failed: {}", git_ref, file_path, String::from_utf8_lossy(&output.stderr));
        None
    }
}
