<div>
  <h1 align="center">voir</h1>
  <h4 align="center">
    SQL over tabular files
  </h4>
</div>

`voir` runs SQL queries against files, starting with CSV. No schema
declaration, no import step, no server.

## Usage

```bash
voir "SELECT region, amount FROM 'testdata/sales.csv' WHERE region = 'EU' LIMIT 5"
voir --explain "SELECT * FROM 'testdata/sales.csv'"
```

Supported for now: `SELECT <cols|*> FROM '<file.csv>' [WHERE <predicate>] [LIMIT n]`,
with `AND`/`OR`/`NOT`, comparisons, arithmetic, and `EXPLAIN`. See `plan.md`
for the full roadmap.

## Development

```bash
cargo check --workspace
cargo nextest run --workspace
cargo deny check
```

## License

This project is licensed under the MIT license and the Apache License 2.0.
