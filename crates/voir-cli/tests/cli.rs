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
fn inner_join_combines_matching_rows() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "SELECT c.name, o.amount FROM '{customers}' c \
             JOIN '{orders}' o ON c.customer_id = o.customer_id \
             ORDER BY o.order_id"
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
    // Carol has no orders and order 104's customer_id (9) matches no
    // customer, so an INNER JOIN drops both — only the three matched pairs
    // survive.
    assert_eq!(lines, vec!["name,amount", "Alice,50", "Alice,75", "Bob,20"]);
}

#[test]
fn left_join_keeps_unmatched_left_row_with_null_right() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "SELECT c.name, o.amount FROM '{customers}' c \
             LEFT JOIN '{orders}' o ON c.customer_id = o.customer_id \
             ORDER BY c.customer_id, o.order_id"
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
    // Carol (customer_id 3) has no matching order — LEFT JOIN keeps her row
    // with the right side NULL-padded, rather than dropping it.
    assert_eq!(
        lines,
        vec!["name,amount", "Alice,50", "Alice,75", "Bob,20", "Carol,"]
    );
}

#[test]
fn full_join_pads_unmatched_rows_on_both_sides() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "SELECT c.name, o.order_id FROM '{customers}' c \
             FULL JOIN '{orders}' o ON c.customer_id = o.customer_id \
             ORDER BY c.customer_id, o.order_id"
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
    // Carol has no order (right side NULL); order 104's customer_id (9)
    // matches no customer (left side NULL) — FULL JOIN keeps both.
    assert!(lines.contains(&"Carol,"));
    assert!(lines.contains(&",104"));
}

#[test]
fn using_join_merges_the_shared_column_unambiguously() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "SELECT customer_id, name FROM '{customers}' c \
             JOIN '{orders}' o USING (customer_id) ORDER BY name, customer_id"
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // `customer_id` is unambiguous post-USING even though both sides have a
    // column by that name — an ON-based join would reject the bare
    // reference as ambiguous.
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["customer_id,name", "1,Alice", "1,Alice", "2,Bob"]
    );
}

#[test]
fn three_file_join_produces_correct_rows() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let regions = testdata("regions.csv");
    let output = voir()
        .arg(format!(
            "SELECT c.name, r.region_name, o.amount \
             FROM '{customers}' c \
             JOIN '{regions}' r ON c.region_id = r.region_id \
             JOIN '{orders}' o ON c.customer_id = o.customer_id \
             ORDER BY o.order_id"
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
    assert_eq!(
        lines,
        vec![
            "name,region_name,amount",
            "Alice,EMEA,50",
            "Alice,EMEA,75",
            "Bob,APAC,20",
        ]
    );
}

#[test]
fn cte_filters_before_the_outer_query_reads_it() {
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "WITH big_orders AS (SELECT * FROM '{orders}' WHERE amount > 50) \
             SELECT order_id FROM big_orders ORDER BY order_id"
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
    assert_eq!(lines, vec!["order_id", "102", "104"]);
}

#[test]
fn union_deduplicates_across_both_branches() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "SELECT customer_id FROM '{customers}' \
             UNION SELECT customer_id FROM '{orders}' ORDER BY customer_id"
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
    // customers has {1,2,3}, orders has {1,1,2,9} — UNION (distinct) merges
    // and dedupes to {1,2,3,9}, not six rows.
    assert_eq!(lines, vec!["customer_id", "1", "2", "3", "9"]);
}

#[test]
fn in_subquery_filters_against_another_file() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    let output = voir()
        .arg(format!(
            "SELECT name FROM '{customers}' \
             WHERE customer_id IN (SELECT customer_id FROM '{orders}') \
             ORDER BY name"
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
    // Carol (customer_id 3) placed no order, so she's excluded.
    assert_eq!(lines, vec!["name", "Alice", "Bob"]);
}

#[test]
fn exists_subquery_is_uncorrelated() {
    let customers = testdata("customers.csv");
    let orders = testdata("orders.csv");
    // `voir` binds EXISTS as an independent, uncorrelated subquery (see
    // plan.md M4 scope) — it can't reference the outer row, so this checks
    // a constant condition rather than a per-row correlation.
    let output = voir()
        .arg(format!(
            "SELECT name FROM '{customers}' \
             WHERE EXISTS (SELECT 1 FROM '{orders}' WHERE amount > 900) \
             ORDER BY name"
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
    assert_eq!(lines, vec!["name", "Alice", "Bob", "Carol"]);
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
