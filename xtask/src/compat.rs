//! Cross-application compatibility validation runner and report generator.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatConfig {
    pub candidate_revision: String,
    pub consumers: Vec<String>,
    pub consumers_dir: PathBuf,
    pub skip_baseline: bool,
    pub dry_run: bool,
    pub all_targets: bool,
    pub quick: bool,
    pub json_output: bool,
    pub output_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainInfo {
    pub rustc: String,
    pub cargo: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub version: String,
    pub candidate_revision: String,
    pub candidate_is_dirty: bool,
    pub timestamp: String,
    pub toolchain: ToolchainInfo,
    pub overall_status: String,
    pub consumers: BTreeMap<String, ConsumerReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerReport {
    pub repository: String,
    pub target_package: String,
    pub consumer_commit: String,
    pub consumer_is_dirty: bool,
    pub original_dependency_pin: String,
    pub candidate_revision: String,
    pub candidate_is_dirty: bool,
    pub resolved_candidate_dependency: String,
    pub baseline_command: Option<String>,
    pub baseline_exit_code: Option<i32>,
    pub baseline_result: String,
    pub baseline_duration_seconds: f64,
    pub candidate_command: String,
    pub candidate_exit_code: Option<i32>,
    pub candidate_result: String,
    pub candidate_duration_seconds: f64,
    pub status: String,
    pub total_duration_seconds: f64,
    pub log_file: Option<String>,
    pub notes: Option<String>,
}

pub struct WorktreeGuard {
    worktrees: Vec<(PathBuf, PathBuf)>, // (repo_dir, worktree_path)
}

impl WorktreeGuard {
    pub fn new() -> Self {
        Self { worktrees: Vec::new() }
    }

    pub fn prune(repo_dir: &Path) {
        let _ = Command::new("git").current_dir(repo_dir).args(["worktree", "prune"]).output();
    }

    pub fn create(&mut self, repo_dir: &Path, worktree_path: &Path, commit_ish: &str) -> Result<(), String> {
        Self::prune(repo_dir);

        if worktree_path.exists() {
            let _ = Self::remove_worktree(repo_dir, worktree_path);
        }
        let output = Command::new("git")
            .current_dir(repo_dir)
            .args(["worktree", "add", "--detach", &worktree_path.to_string_lossy(), commit_ish])
            .output()
            .map_err(|e| format!("Failed to spawn git worktree add: {e}"))?;

        if !output.status.success() {
            return Err(format!("git worktree add failed in {}: {}", repo_dir.display(), String::from_utf8_lossy(&output.stderr).trim()));
        }

        self.worktrees.push((repo_dir.to_path_buf(), worktree_path.to_path_buf()));
        Ok(())
    }

    fn remove_worktree(repo_dir: &Path, worktree_path: &Path) -> Result<(), String> {
        let _ = Command::new("git").current_dir(repo_dir).args(["worktree", "remove", "--force", &worktree_path.to_string_lossy()]).output();

        if worktree_path.exists() {
            let _ = fs::remove_dir_all(worktree_path);
        }
        Self::prune(repo_dir);
        Ok(())
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        for (repo_dir, worktree_path) in self.worktrees.drain(..) {
            let _ = Self::remove_worktree(&repo_dir, &worktree_path);
        }
    }
}

pub struct ConsumerSpec {
    pub name: &'static str,
    pub default_repo_dir: &'static str,
    pub ui_crate: &'static str,
}

pub const KNOWN_CONSUMERS: &[ConsumerSpec] = &[
    ConsumerSpec { name: "photocraft", default_repo_dir: "photocraft", ui_crate: "photocraft-ui-egui" },
    ConsumerSpec { name: "vectorcraft", default_repo_dir: "vectorcraft", ui_crate: "vectorcraft-ui-egui" },
    ConsumerSpec { name: "designcraft", default_repo_dir: "designcraft", ui_crate: "designcraft-ui-egui" },
    ConsumerSpec { name: "pdfcraft", default_repo_dir: "pdfcraft", ui_crate: "pdfcraft-ui-egui" },
    ConsumerSpec { name: "lightcraft", default_repo_dir: "lightcraft", ui_crate: "lightcraft-ui-egui" },
    ConsumerSpec { name: "filmcraft", default_repo_dir: "filmcraft", ui_crate: "filmcraft-ui-egui" },
    ConsumerSpec { name: "effectcraft", default_repo_dir: "effectcraft", ui_crate: "effectcraft-ui-egui" },
    ConsumerSpec { name: "cadcraft", default_repo_dir: "cadcraft", ui_crate: "cadcraft-ui-egui" },
    ConsumerSpec { name: "soundcraft", default_repo_dir: "soundcraft", ui_crate: "soundcraft-ui-egui" },
];

pub fn get_toolchain_info() -> ToolchainInfo {
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into());

    let cargo = Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into());

    let target = Command::new("rustc")
        .args(["-vV"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            s.lines().find(|l| l.starts_with("host:")).map(|l| l.trim_start_matches("host:").trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into());

    ToolchainInfo { rustc, cargo, target }
}

pub fn check_git_dirty(dir: &Path) -> bool {
    Command::new("git").current_dir(dir).args(["status", "--porcelain"]).output().map(|o| !o.stdout.is_empty()).unwrap_or(false)
}

pub fn resolve_commit(repo_dir: &Path, commit_ish: &str) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(repo_dir)
        .args(["rev-parse", "--verify", &format!("{commit_ish}^{{commit}}")])
        .output()
        .map_err(|e| format!("git rev-parse failed to spawn: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "Commit `{commit_ish}` could not be resolved in {}: {}",
            repo_dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn inspect_resolved_craft_ui(dir: &Path, target_pkg: Option<&str>, config_override: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(dir);
    cmd.arg("tree");
    if let Some(pkg) = target_pkg {
        cmd.args(["-p", pkg]);
    }
    cmd.args(["-i", "craft-ui"]);
    if let Some(cfg) = config_override {
        cmd.args(["--config", cfg]);
    }

    let out = cmd.output().map_err(|e| format!("cargo tree failed to spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo tree returned error: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let Some(first_line) = stdout.lines().find(|l| l.contains("craft-ui")) {
        Ok(first_line.trim().to_string())
    } else {
        Err("craft-ui not found in cargo tree output".into())
    }
}

fn extract_test_summary(stdout: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(stdout);
    for line in s.lines().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:") {
            return Some(trimmed.to_string());
        }
    }
    None
}

pub fn run_compat(root_dir: &Path, config: CompatConfig) -> Result<bool, String> {
    println!("Craft Suite Compatibility Validation");
    println!("=======================================================\n");

    let candidate_sha = resolve_commit(root_dir, &config.candidate_revision)?;
    let candidate_is_dirty = check_git_dirty(root_dir);

    println!("Target shared library: craft-libs ({})", candidate_sha);
    if candidate_is_dirty {
        println!("Working tree status:   DIRTY (uncommitted local changes present)");
    } else {
        println!("Working tree status:   CLEAN");
    }

    let toolchain = get_toolchain_info();
    println!("Rust toolchain:        {}", toolchain.rustc);
    println!("Cargo version:         {}", toolchain.cargo);
    println!("Host target:           {}", toolchain.target);
    println!("Consumers to test:     {}\n", config.consumers.join(", "));

    let mut guard = WorktreeGuard::new();
    let compat_root = root_dir.join("target").join("compat");
    let worktrees_dir = compat_root.join("worktrees");
    let logs_dir = compat_root.join("logs");
    fs::create_dir_all(&worktrees_dir).map_err(|e| format!("create worktrees dir: {e}"))?;
    fs::create_dir_all(&logs_dir).map_err(|e| format!("create logs dir: {e}"))?;

    // Prune stale worktrees from target/compat/worktrees
    WorktreeGuard::prune(root_dir);

    // Create isolated worktree for candidate craft-libs revision
    let candidate_craft_libs_dir = worktrees_dir.join(format!("craft-libs-{}", &candidate_sha[0..8]));
    println!("Creating candidate craft-libs worktree at {}", candidate_craft_libs_dir.display());
    guard.create(root_dir, &candidate_craft_libs_dir, &candidate_sha)?;
    let candidate_ui_crate_path = candidate_craft_libs_dir.join("crates").join("ui");

    if !candidate_ui_crate_path.exists() {
        return Err(format!("crates/ui does not exist in candidate revision {}", candidate_sha));
    }

    let patch_config = format!("patch.\"https://github.com/storytold/craft-libs\".craft-ui.path=\"{}\"", candidate_ui_crate_path.display());

    let mut consumer_reports = BTreeMap::new();
    let mut any_regression = false;

    for consumer_name in &config.consumers {
        let start_time = Instant::now();
        println!("-------------------------------------------------------");
        println!("Validating consumer: {consumer_name}");
        println!("-------------------------------------------------------");

        let spec = match KNOWN_CONSUMERS.iter().find(|c| c.name == consumer_name) {
            Some(s) => s,
            None => {
                eprintln!("Unknown consumer: {consumer_name} (skipping)");
                consumer_reports.insert(
                    consumer_name.clone(),
                    ConsumerReport {
                        repository: consumer_name.clone(),
                        target_package: "unknown".into(),
                        consumer_commit: "unknown".into(),
                        consumer_is_dirty: false,
                        original_dependency_pin: "unknown".into(),
                        candidate_revision: candidate_sha.clone(),
                        candidate_is_dirty,
                        resolved_candidate_dependency: "unknown".into(),
                        baseline_command: None,
                        baseline_exit_code: None,
                        baseline_result: "SKIP".into(),
                        baseline_duration_seconds: 0.0,
                        candidate_command: "none".into(),
                        candidate_exit_code: None,
                        candidate_result: "SKIP".into(),
                        candidate_duration_seconds: 0.0,
                        status: "SKIPPED".into(),
                        total_duration_seconds: 0.0,
                        log_file: None,
                        notes: Some(format!("Unknown consumer name `{consumer_name}`")),
                    },
                );
                continue;
            }
        };

        let repo_dir = config.consumers_dir.join(spec.default_repo_dir);
        if !repo_dir.exists() {
            println!("  [SKIP] Consumer checkout not found at {}", repo_dir.display());
            consumer_reports.insert(
                consumer_name.clone(),
                ConsumerReport {
                    repository: spec.default_repo_dir.into(),
                    target_package: spec.ui_crate.into(),
                    consumer_commit: "missing".into(),
                    consumer_is_dirty: false,
                    original_dependency_pin: "missing".into(),
                    candidate_revision: candidate_sha.clone(),
                    candidate_is_dirty,
                    resolved_candidate_dependency: "missing".into(),
                    baseline_command: None,
                    baseline_exit_code: None,
                    baseline_result: "SKIP".into(),
                    baseline_duration_seconds: 0.0,
                    candidate_command: "none".into(),
                    candidate_exit_code: None,
                    candidate_result: "SKIP".into(),
                    candidate_duration_seconds: 0.0,
                    status: "SKIPPED".into(),
                    total_duration_seconds: 0.0,
                    log_file: None,
                    notes: Some(format!("Repository directory not found at {}", repo_dir.display())),
                },
            );
            continue;
        }

        let consumer_is_dirty = check_git_dirty(&repo_dir);
        let baseline_commit = match resolve_commit(&repo_dir, "HEAD") {
            Ok(c) => c,
            Err(e) => {
                println!("  [ERROR] Failed to get HEAD of {}: {e}", repo_dir.display());
                consumer_reports.insert(
                    consumer_name.clone(),
                    ConsumerReport {
                        repository: spec.default_repo_dir.into(),
                        target_package: spec.ui_crate.into(),
                        consumer_commit: "unknown".into(),
                        consumer_is_dirty,
                        original_dependency_pin: "unknown".into(),
                        candidate_revision: candidate_sha.clone(),
                        candidate_is_dirty,
                        resolved_candidate_dependency: "unknown".into(),
                        baseline_command: None,
                        baseline_exit_code: None,
                        baseline_result: "INFRASTRUCTURE_FAILURE".into(),
                        baseline_duration_seconds: 0.0,
                        candidate_command: "none".into(),
                        candidate_exit_code: None,
                        candidate_result: "INFRASTRUCTURE_FAILURE".into(),
                        candidate_duration_seconds: 0.0,
                        status: "INFRASTRUCTURE_FAILURE".into(),
                        total_duration_seconds: start_time.elapsed().as_secs_f64(),
                        log_file: None,
                        notes: Some(format!("Failed to resolve HEAD: {e}")),
                    },
                );
                continue;
            }
        };

        // 1. Inspect and test Baseline
        let baseline_resolved = match inspect_resolved_craft_ui(&repo_dir, Some(spec.ui_crate), None) {
            Ok(r) => r,
            Err(e) => {
                println!("  [WARN] Failed to inspect baseline craft-ui: {e}");
                format!("INSPECT_FAILED ({e})")
            }
        };

        println!("  Baseline commit:       {}{}", &baseline_commit[0..8], if consumer_is_dirty { " (DIRTY)" } else { "" });
        println!("  Baseline dependency:   {}", baseline_resolved);

        let shared_target_dir = repo_dir.join("target");
        let mut test_args: Vec<String> = if config.all_targets {
            vec!["test".into(), "-p".into(), spec.ui_crate.to_string()]
        } else {
            vec!["test".into(), "-p".into(), spec.ui_crate.to_string(), "--lib".into()]
        };

        if config.quick {
            test_args.extend(["--config".into(), "profile.dev.package.\"*\".opt-level=0".into()]);
        }

        let baseline_start = Instant::now();
        let mut baseline_cmd_str = None;
        let mut baseline_code = None;
        let baseline_result = if config.skip_baseline {
            println!("  Baseline execution:    SKIPPED (--skip-baseline)");
            "SKIPPED".to_string()
        } else if config.dry_run {
            println!("  Baseline execution:    SKIPPED (dry run)");
            "DRY_RUN".to_string()
        } else {
            let cmd_text = format!("cargo {}", test_args.join(" "));
            baseline_cmd_str = Some(cmd_text);
            print!("  Running baseline tests: ");
            let out = Command::new("cargo").current_dir(&repo_dir).args(&test_args).env("CARGO_TARGET_DIR", &shared_target_dir).output();

            match out {
                Ok(o) if o.status.success() => {
                    if let Some(summary) = extract_test_summary(&o.stdout) {
                        println!("PASS ({summary})");
                    } else {
                        println!("PASS");
                    }
                    baseline_code = o.status.code();
                    fs::write(logs_dir.join(format!("{consumer_name}-baseline.log")), &o.stdout).ok();
                    "PASS".to_string()
                }
                Ok(o) => {
                    let mut log = o.stdout;
                    log.extend_from_slice(&o.stderr);
                    if let Some(summary) = extract_test_summary(&log) {
                        println!("FAIL ({summary})");
                    } else {
                        println!("FAIL (exit code: {:?})", o.status.code());
                    }
                    baseline_code = o.status.code();
                    fs::write(logs_dir.join(format!("{consumer_name}-baseline.log")), &log).ok();
                    "FAIL".to_string()
                }
                Err(e) => {
                    println!("INFRASTRUCTURE_FAILURE ({e})");
                    "INFRASTRUCTURE_FAILURE".to_string()
                }
            }
        };
        let baseline_duration = baseline_start.elapsed().as_secs_f64();

        // 2. Candidate isolated test
        let cand_worktree_dir = worktrees_dir.join(format!("{consumer_name}-cand-{}", &candidate_sha[0..8]));
        println!("  Creating candidate worktree: {}", cand_worktree_dir.display());
        if let Err(e) = guard.create(&repo_dir, &cand_worktree_dir, "HEAD") {
            println!("  [ERROR] Worktree creation failed: {e}");
            consumer_reports.insert(
                consumer_name.clone(),
                ConsumerReport {
                    repository: spec.default_repo_dir.into(),
                    target_package: spec.ui_crate.into(),
                    consumer_commit: baseline_commit,
                    consumer_is_dirty,
                    original_dependency_pin: baseline_resolved,
                    candidate_revision: candidate_sha.clone(),
                    candidate_is_dirty,
                    resolved_candidate_dependency: "worktree-creation-failed".into(),
                    baseline_command: baseline_cmd_str,
                    baseline_exit_code: baseline_code,
                    baseline_result,
                    baseline_duration_seconds: baseline_duration,
                    candidate_command: "none".into(),
                    candidate_exit_code: None,
                    candidate_result: "INFRASTRUCTURE_FAILURE".into(),
                    candidate_duration_seconds: 0.0,
                    status: "INFRASTRUCTURE_FAILURE".into(),
                    total_duration_seconds: start_time.elapsed().as_secs_f64(),
                    log_file: None,
                    notes: Some(format!("Worktree creation failed: {e}")),
                },
            );
            continue;
        }

        // 3. Strict provenance verification: target package must resolve to candidate path
        let expected_snippet = candidate_ui_crate_path.to_string_lossy();
        let candidate_resolved = match inspect_resolved_craft_ui(&cand_worktree_dir, Some(spec.ui_crate), Some(&patch_config)) {
            Ok(r) => {
                if !r.contains(expected_snippet.as_ref()) {
                    println!("  [ERROR] Provenance mismatch: expected '{}' in '{}'", expected_snippet, r);
                    format!("PROVENANCE_MISMATCH ({r})")
                } else {
                    r
                }
            }
            Err(e) => {
                println!("  [ERROR] Candidate dependency resolution failed: {e}");
                format!("RESOLUTION_FAILED ({e})")
            }
        };
        println!("  Candidate dependency:  {}", candidate_resolved);

        let candidate_start = Instant::now();
        let mut cand_args = test_args.clone();
        cand_args.extend(["--config".into(), patch_config.clone()]);
        let cand_cmd_text = format!("cargo {}", cand_args.join(" "));
        let mut candidate_code = None;
        let mut log_path = None;

        let candidate_result = if candidate_resolved.starts_with("PROVENANCE_MISMATCH") || candidate_resolved.starts_with("RESOLUTION_FAILED") {
            "INFRASTRUCTURE_FAILURE".to_string()
        } else if config.dry_run {
            println!("  Candidate execution:   SKIPPED (dry run)");
            "DRY_RUN".to_string()
        } else {
            print!("  Running candidate tests: ");
            let out = Command::new("cargo").current_dir(&cand_worktree_dir).args(&cand_args).env("CARGO_TARGET_DIR", &shared_target_dir).output();

            let log_file_name = format!("{consumer_name}-candidate.log");
            let target_log = logs_dir.join(&log_file_name);
            log_path = Some(target_log.to_string_lossy().to_string());

            match out {
                Ok(o) if o.status.success() => {
                    if let Some(summary) = extract_test_summary(&o.stdout) {
                        println!("PASS ({summary})");
                    } else {
                        println!("PASS");
                    }
                    candidate_code = o.status.code();
                    fs::write(&target_log, &o.stdout).ok();
                    "PASS".to_string()
                }
                Ok(o) => {
                    let mut log = o.stdout;
                    log.extend_from_slice(&o.stderr);
                    if let Some(summary) = extract_test_summary(&log) {
                        println!("FAIL ({summary})");
                    } else {
                        println!("FAIL (exit code: {:?})", o.status.code());
                    }
                    candidate_code = o.status.code();
                    fs::write(&target_log, &log).ok();
                    "FAIL".to_string()
                }
                Err(e) => {
                    println!("INFRASTRUCTURE_FAILURE ({e})");
                    "INFRASTRUCTURE_FAILURE".to_string()
                }
            }
        };
        let candidate_duration = candidate_start.elapsed().as_secs_f64();

        // Classify outcome
        let status = classify_status(&baseline_result, &candidate_result);
        if status == "CANDIDATE_REGRESSION" {
            any_regression = true;
        }

        let total_duration = start_time.elapsed().as_secs_f64();
        println!("  Outcome:               {}\n", status);

        let notes = if status == "FIXED_IN_CANDIDATE" {
            Some("Observed outcome: baseline failed on committed pin and passed on candidate; causality should be verified against environmental variance or test flakiness.".into())
        } else {
            None
        };

        consumer_reports.insert(
            consumer_name.clone(),
            ConsumerReport {
                repository: spec.default_repo_dir.into(),
                target_package: spec.ui_crate.into(),
                consumer_commit: baseline_commit,
                consumer_is_dirty,
                original_dependency_pin: baseline_resolved,
                candidate_revision: candidate_sha.clone(),
                candidate_is_dirty,
                resolved_candidate_dependency: candidate_resolved,
                baseline_command: baseline_cmd_str,
                baseline_exit_code: baseline_code,
                baseline_result,
                baseline_duration_seconds: baseline_duration,
                candidate_command: cand_cmd_text,
                candidate_exit_code: candidate_code,
                candidate_result,
                candidate_duration_seconds: candidate_duration,
                status: status.to_string(),
                total_duration_seconds: total_duration,
                log_file: log_path,
                notes,
            },
        );
    }

    let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".into(),
    };

    let overall_status = if any_regression {
        "FAIL_REGRESSION"
    } else if consumer_reports.values().any(|r| r.status == "INFRASTRUCTURE_FAILURE") {
        "INFRASTRUCTURE_FAILURE"
    } else {
        "PASS"
    };

    let report = CompatibilityReport {
        version: "1.0".into(),
        candidate_revision: candidate_sha,
        candidate_is_dirty,
        timestamp,
        toolchain,
        overall_status: overall_status.into(),
        consumers: consumer_reports,
    };

    let report_json = serde_json::to_string_pretty(&report).map_err(|e| format!("serialize report: {e}"))?;
    let report_path = config.output_file.unwrap_or_else(|| compat_root.join("report.json"));
    fs::write(&report_path, &report_json).map_err(|e| format!("write report: {e}"))?;

    println!("=======================================================");
    println!("Compatibility Summary");
    println!("=======================================================");
    println!("{:<14} {:<10} {:<10} {:<22}", "Consumer", "Baseline", "Candidate", "Status");
    println!("{:-<14} {:-<10} {:-<10} {:-<22}", "", "", "", "");
    for (name, r) in &report.consumers {
        println!("{:<14} {:<10} {:<10} {:<22}", name, r.baseline_result, r.candidate_result, r.status);
    }
    println!("\nReport written to: {}", report_path.display());

    if config.json_output {
        println!("\n```json\n{report_json}\n```");
    }

    Ok(!any_regression && overall_status == "PASS")
}

pub fn classify_status(baseline_result: &str, candidate_result: &str) -> &'static str {
    match (baseline_result, candidate_result) {
        ("PASS", "PASS") => "COMPATIBLE",
        ("PASS", "FAIL") => "CANDIDATE_REGRESSION",
        ("FAIL", "FAIL") => "BASELINE_FAILURE",
        ("FAIL", "PASS") => "FIXED_IN_CANDIDATE",
        ("SKIPPED", "PASS") => "COMPATIBLE",
        ("SKIPPED", "FAIL") => "CANDIDATE_REGRESSION",
        ("SKIPPED", "SKIPPED") => "SKIPPED",
        ("DRY_RUN", "DRY_RUN") | ("SKIPPED", "DRY_RUN") => "PROVENANCE_VERIFIED",
        (b, c) if b == "INFRASTRUCTURE_FAILURE" || c == "INFRASTRUCTURE_FAILURE" => "INFRASTRUCTURE_FAILURE",
        (b, c) if b == "INCOMPLETE" || c == "INCOMPLETE" => "INCOMPLETE",
        _ => "INFRASTRUCTURE_FAILURE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolchain_info_non_empty() {
        let info = get_toolchain_info();
        assert!(!info.rustc.is_empty());
        assert!(!info.cargo.is_empty());
        assert!(!info.target.is_empty());
    }

    #[test]
    fn test_resolve_valid_and_invalid_commit() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let valid = resolve_commit(&root, "HEAD");
        assert!(valid.is_ok());
        let sha = valid.unwrap();
        assert_eq!(sha.len(), 40);

        let invalid = resolve_commit(&root, "0000000000000000000000000000000000000000");
        assert!(invalid.is_err());
    }

    #[test]
    fn test_classify_status_matrix() {
        assert_eq!(classify_status("PASS", "PASS"), "COMPATIBLE");
        assert_eq!(classify_status("PASS", "FAIL"), "CANDIDATE_REGRESSION");
        assert_eq!(classify_status("FAIL", "FAIL"), "BASELINE_FAILURE");
        assert_eq!(classify_status("FAIL", "PASS"), "FIXED_IN_CANDIDATE");
        assert_eq!(classify_status("SKIPPED", "PASS"), "COMPATIBLE");
        assert_eq!(classify_status("SKIPPED", "FAIL"), "CANDIDATE_REGRESSION");
        assert_eq!(classify_status("DRY_RUN", "DRY_RUN"), "PROVENANCE_VERIFIED");
        assert_eq!(classify_status("SKIPPED", "DRY_RUN"), "PROVENANCE_VERIFIED");
        assert_eq!(classify_status("INFRASTRUCTURE_FAILURE", "PASS"), "INFRASTRUCTURE_FAILURE");
        assert_eq!(classify_status("PASS", "INFRASTRUCTURE_FAILURE"), "INFRASTRUCTURE_FAILURE");
        assert_eq!(classify_status("ERROR", "PASS"), "INFRASTRUCTURE_FAILURE");
    }

    #[test]
    fn test_worktree_guard_lifecycle() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let wt = std::env::temp_dir().join(format!("craft-wt-test-{}", std::process::id()));
        if wt.exists() {
            let _ = fs::remove_dir_all(&wt);
        }

        let mut guard = WorktreeGuard::new();
        let created = guard.create(&root, &wt, "HEAD");
        assert!(created.is_ok());
        assert!(wt.exists());

        drop(guard);
        assert!(!wt.exists());
    }
}
