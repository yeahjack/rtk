//! Real Git coverage for the opt-in commit preview budget.

use std::path::Path;
use std::process::Output;

mod common;

const BUDGET_VAR: &str = "RTK_GIT_SHOW_MAX_LINES";
const FILE: &str = "sample with spaces.txt";

fn fixture() -> tempfile::TempDir {
    let repo = common::temp_git_repo();
    let long: String = (0..180)
        .map(|i| format!("line {i:03}: {}\n", "fixture ".repeat(15)))
        .collect();
    std::fs::write(repo.path().join(FILE), long).expect("write fixture");
    let add = git(repo.path(), &["add", "--", FILE]);
    assert!(add.status.success(), "{add:?}");
    let commit = git(
        repo.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "add fixture",
        ],
    );
    assert!(commit.status.success(), "{commit:?}");
    repo
}

fn git(repo: &Path, args: &[&str]) -> Output {
    common::native_command("git")
        .current_dir(repo)
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .expect("run native git")
}

fn show(repo: &Path, budget: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = common::rtk_command();
    if let Some(budget) = budget {
        cmd.env(BUDGET_VAR, budget);
    }
    cmd.current_dir(repo)
        .env("LC_ALL", "C")
        .args(["git", "show"])
        .args(args)
        .output()
        .expect("run rtk git show")
}

#[test]
fn bounds_a_commit_preview_and_leaves_unset_or_invalid_budgets_alone() {
    let repo = fixture();
    let args = &["HEAD", "--", FILE];
    let default = show(repo.path(), None, args);
    assert!(default.status.success(), "{default:?}");
    for ignored in ["0", "-1", "not-a-number", "184467440737095516160"] {
        let output = show(repo.path(), Some(ignored), args);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, default.stdout, "budget {ignored:?}");
    }
    for limit in [1, 3, 20, 500] {
        let output = show(repo.path(), Some(&limit.to_string()), args);
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).expect("preview is UTF-8");
        assert!(text.lines().count() <= limit, "budget {limit}: {text}");
        assert!(text.contains("rtk proxy git show HEAD -- 'sample with spaces.txt'"));
        assert!(!text.contains("rtk git diff --no-compact"));
        assert!(!text.contains("line 179"));
        if limit == 20 {
            assert!(text.contains("line 000"));
            assert!(text.len() < default.stdout.len());
        }
    }
}

#[cfg(unix)]
#[test]
fn displayed_recovery_keeps_repository_and_pathspec_and_bypasses_the_budget() {
    let repo = fixture();
    let output = common::rtk_command()
        .env(BUDGET_VAR, "12")
        .env("LC_ALL", "C")
        .args(["git", "-C"])
        .arg(repo.path())
        .args(["show", "HEAD", "--", FILE])
        .output()
        .expect("run rtk git show from outside the repo");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).expect("preview is UTF-8");
    let hint = text
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("[full diff: "))
        .and_then(|line| line.strip_suffix(']'))
        .expect("last line names the full diff");
    let args = hint
        .strip_prefix("rtk ")
        .expect("hint bypasses RTK filtering");

    let mut cmd = std::process::Command::new("/bin/sh");
    common::isolate_rtk(&mut cmd);
    let rtk = common::rtk_command();
    // Execute the displayed command with the tested binary (not a system install).
    let recovered = cmd
        .env(BUDGET_VAR, "12")
        .env("LC_ALL", "C")
        .env("RTK_TEST_BIN", rtk.get_program())
        .arg("-c")
        .arg(format!("\"$RTK_TEST_BIN\" {args}"))
        .output()
        .expect("follow displayed recovery");
    assert!(recovered.status.success(), "{recovered:?}");
    let native = git(repo.path(), &["show", "HEAD", "--", FILE]);
    assert_eq!(recovered.stdout, native.stdout);
    assert!(String::from_utf8_lossy(&recovered.stdout).contains("line 179"));
}

#[test]
fn leaves_special_formats_blobs_and_native_errors_outside_the_budget() {
    let repo = fixture();
    for args in [
        &["--name-only", "--format=%H", "HEAD"][..],
        &["--raw", "-z", "HEAD"],
        &["--word-diff=porcelain", "HEAD"],
        &["--no-patch", "HEAD"],
        &["HEAD:sample with spaces.txt"],
        &["missing-rtk-test-revision", "--"],
    ] {
        let default = show(repo.path(), None, args);
        let budgeted = show(repo.path(), Some("1"), args);
        assert_eq!(budgeted.status.code(), default.status.code(), "{args:?}");
        assert_eq!(budgeted.stdout, default.stdout, "{args:?}");
    }
    let args = ["missing-rtk-test-revision", "--"];
    let native = git(repo.path(), &["show", args[0], args[1]]);
    let budgeted = show(repo.path(), Some("1"), &args);
    assert!(!native.status.success());
    assert_eq!(budgeted.status.code(), native.status.code());
    let diagnostic = String::from_utf8_lossy(&native.stderr);
    assert!(!diagnostic.trim().is_empty());
    assert!(String::from_utf8_lossy(&budgeted.stderr).contains(diagnostic.trim()));

    let native = git(repo.path(), &["show", "--check", "HEAD"]);
    let budgeted = show(repo.path(), Some("1"), &["--check", "HEAD"]);
    assert_eq!(native.status.code(), Some(2), "{native:?}");
    assert_eq!(budgeted.status.code(), native.status.code());
    assert_eq!(budgeted.stdout, native.stdout);
    assert!(String::from_utf8_lossy(&budgeted.stdout).contains("trailing whitespace"));
}

#[test]
fn budget_does_not_leak_into_graphite_dispatch() {
    let repo = fixture();
    let run = |budget| {
        let mut cmd = common::rtk_command();
        if let Some(budget) = budget {
            cmd.env(BUDGET_VAR, budget);
        }
        cmd.current_dir(repo.path())
            .args(["gt", "show", "HEAD"])
            .output()
            .expect("run rtk gt show")
    };
    let default = run(None);
    let budgeted = run(Some("1"));
    assert!(default.status.success(), "{default:?}");
    assert_eq!(budgeted.status.code(), default.status.code());
    assert_eq!(budgeted.stdout, default.stdout);
}
