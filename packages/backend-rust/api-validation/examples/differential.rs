//! Reads AJV reference cases as JSON lines. No database or application process.
use misskey_api_validation::{Schema, Value};
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};

#[derive(Deserialize)]
struct Batch {
    name: String,
    schema: serde_json::Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    input: Value,
    expected: serde_json::Value,
}

fn main() {
    let mut count = 0;
    let mut failures = 0;
    for line in io::stdin().lock().lines() {
        let batch: Batch =
            serde_json::from_str(&line.expect("input line")).expect("reference batch");
        let schema = Schema::compile(batch.schema).expect("compile reference schema");
        for case in batch.cases {
            count += 1;
            let mut value = case.input;
            let result = schema.validate(&mut value);
            let actual = json!({"valid": result.is_ok(), "error": result.err(), "value": value});
            if actual != case.expected {
                failures += 1;
                if failures <= 100 {
                    println!(
                        "{}",
                        json!({"name":batch.name,"id":case.id,"actual":actual,"expected":case.expected})
                    );
                }
            }
        }
    }
    eprintln!("Compared {count} cases; {failures} mismatches");
    if failures != 0 {
        std::process::exit(1)
    }
}
