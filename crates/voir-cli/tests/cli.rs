use std::process::Command;

fn voir() -> Command {
    Command::new(env!("CARGO_BIN_EXE_voir"))
}

fn testdata(name: &str) -> String {
    format!("{}/../../testdata/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn selects_and_filters_a_real_file() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!("SELECT region, amount FROM '{path}' WHERE region = 'EU' LIMIT 2"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("region"));
    assert!(stdout.contains("EU"));
    assert!(!stdout.contains("US"));
}

#[test]
fn explain_prints_a_plan_without_running() {
    let path = testdata("sales.csv");
    let output = voir().arg("--explain").arg(format!("SELECT * FROM '{path}'")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Scan"));
    assert!(stdout.contains("Projection"));
}

#[test]
fn unknown_column_is_a_plan_error() {
    let path = testdata("sales.csv");
    let output = voir().arg(format!("SELECT nope FROM '{path}'")).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nope"));
}
