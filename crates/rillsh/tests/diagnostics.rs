//! Reviewed public diagnostics, including original source context across calls and cleanup.
use std::process::Command;

fn diagnostic(source: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["--color=never", "-c", source])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn incomplete_expression() {
    insta::assert_snapshot!(diagnostic("let answer ="));
}

#[test]
fn closure_failure_points_to_its_definition() {
    insta::assert_snapshot!(diagnostic(
        "let divide = { denominator => 12.0 / denominator }\ndivide 0.0"
    ));
}

#[test]
fn unicode_before_failure_preserves_source_columns() {
    insta::assert_snapshot!(diagnostic("let greeting = \"\u{1f30a}\"; greeting + 1"));
}

#[test]
fn cleanup_failure_retains_primary_diagnostic() {
    insta::assert_snapshot!(diagnostic(
        r#"seq.produce {
  acquire: { () => 0 },
  step: { _ => raise (error "StepFailure" "cannot produce a value") },
  release: { _ _ => raise (error "ReleaseFailure" "cannot finish cleanup") }
} |> collect"#
    ));
}
