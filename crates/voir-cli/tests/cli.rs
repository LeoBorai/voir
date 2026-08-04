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
        .arg(format!(
            "SELECT region, amount FROM '{path}' WHERE region = 'EU' LIMIT 2"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("region"));
    assert!(stdout.contains("EU"));
    assert!(!stdout.contains("US"));
}

#[test]
fn explain_prints_a_plan_without_running() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg("--explain")
        .arg(format!("SELECT * FROM '{path}'"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Scan"));
    assert!(stdout.contains("Projection"));
}

#[test]
fn unknown_column_is_a_plan_error() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!("SELECT nope FROM '{path}'"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nope"));
}

#[test]
fn group_by_aggregates_and_orders_results() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            "SELECT region, COUNT(*) AS n, SUM(amount) AS total \
             FROM '{path}' GROUP BY region ORDER BY region"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "region,n,total");
    assert_eq!(lines[1], "APAC,1,102.25");
    assert_eq!(lines[2], "EU,3,165.5");
    // US's total (89.99 + 60.10) is asserted with a tolerance rather than an
    // exact string: those two decimals aren't exactly representable in
    // binary floating point, so the printed sum isn't guaranteed to be the
    // literal shortest-round-trip form of "150.09".
    let us: Vec<&str> = lines[3].split(',').collect();
    assert_eq!(&us[..2], ["US", "2"]);
    let total: f64 = us[2].parse().unwrap();
    assert!((total - 150.09).abs() < 0.001, "total was {total}");
}

#[test]
fn having_filters_on_aggregate_result() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            "SELECT region, COUNT(*) AS n FROM '{path}' \
             GROUP BY region HAVING COUNT(*) > 1 ORDER BY region"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("APAC"));
    assert!(stdout.contains("EU,3"));
    assert!(stdout.contains("US,2"));
}

#[test]
fn bare_column_outside_group_by_is_a_plan_error() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            "SELECT region, product FROM '{path}' GROUP BY region"
        ))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GROUP BY"));
}

#[test]
fn distinct_deduplicates_rows() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            "SELECT DISTINCT region FROM '{path}' ORDER BY region"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["region", "APAC", "EU", "US"]);
}

#[test]
fn order_by_desc_with_limit_and_offset() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            // `amount > 0` drops the one NULL row so the DESC/NULL-ordering
            // rule (NULLS FIRST) doesn't affect which row OFFSET lands on.
            "SELECT product, amount FROM '{path}' WHERE amount > 0 \
             ORDER BY amount DESC LIMIT 1 OFFSET 1"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Sorted desc by amount: 120.50, 102.25, 89.99, 60.10, 45.00.
    // OFFSET 1 skips 120.50, LIMIT 1 keeps only 102.25.
    assert!(stdout.contains("102.25"));
    assert!(!stdout.contains("120.5"));
}

#[test]
fn case_and_cast_expressions() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            "SELECT region, CASE WHEN amount > 100 THEN 'big' ELSE 'small' END AS size, \
             CAST(amount AS INT) AS whole \
             FROM '{path}' WHERE amount > 0 ORDER BY region, amount"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("big"));
    assert!(stdout.contains("small"));
    assert!(stdout.contains(",120"));
}

#[test]
fn scalar_functions() {
    let path = testdata("sales.csv");
    let output = voir()
        .arg(format!(
            "SELECT UPPER(region) AS r, ROUND(amount, 1) AS a \
             FROM '{path}' WHERE region = 'EU' AND amount > 0 ORDER BY amount"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("EU,45"));
    assert!(stdout.contains("EU,120.5"));
    assert!(!stdout.contains("eu"));
}
