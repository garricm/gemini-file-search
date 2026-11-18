use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gfs"))
        .args(args)
        .env_remove("GEMINI_FILE_SEARCH_API_KEY")
        .env_remove("GEMINI_UPLOAD_ROOT")
        .output()
        .unwrap()
}

#[test]
fn help_works_without_credentials_and_lists_interactive_and_script_modes() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "tui",
        "stores",
        "documents",
        "upload",
        "ask",
        "operation",
        "--demo",
        "--json",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
}

#[test]
fn redirected_terminal_mode_and_invalid_demo_modes_fail_without_network() {
    let output = run(&["--demo"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("interactive terminal")
    );
    for arguments in [&["stores", "list", "--demo"][..], &["--json", "tui"][..]] {
        assert!(!run(arguments).status.success());
    }
}

#[test]
fn json_mode_emits_parseable_errors_on_stderr_only() {
    let output = run(&["stores", "list", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("GEMINI_FILE_SEARCH_API_KEY")
    );
}

#[test]
fn unsafe_resource_names_and_noninteractive_deletion_never_reach_google() {
    let root = tempfile::tempdir().unwrap();
    for (arguments, expected) in [
        (
            vec!["documents", "delete", "https://evil.example/steal", "--yes"],
            "resource name",
        ),
        (
            vec!["stores", "delete", "fileSearchStores/test"],
            "Use --yes",
        ),
        (
            vec!["operation", "fileSearchStores/test/../escape"],
            "operation name",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gfs"))
            .args(arguments)
            .env("GEMINI_FILE_SEARCH_API_KEY", "synthetic-cli-test-key")
            .env("GEMINI_UPLOAD_ROOT", root.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("synthetic-cli-test-key"));
    }
}

#[test]
fn timeout_is_validated_before_configuration_or_network() {
    let output = run(&[
        "upload",
        "guide.md",
        "--store",
        "fileSearchStores/test",
        "--wait",
        "--timeout",
        "0",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("invalid value")
    );
}

#[test]
fn uploading_an_empty_directory_reports_no_eligible_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("empty.md"), "").unwrap();
    std::fs::write(root.path().join("unsupported.csv"), "data").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_gfs"))
        .args(["upload", ".", "--store", "fileSearchStores/test", "--json"])
        .env("GEMINI_FILE_SEARCH_API_KEY", "synthetic-cli-test-key")
        .env("GEMINI_UPLOAD_ROOT", root.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("no eligible files")
    );
}
