//! Collects results and prints a table with pass/fail against each budget.

pub struct Report {
    rows: Vec<Row>,
    filter: Option<String>,
}

struct Row {
    name: String,
    median: f64,
    p95: f64,
    budget: f64,
}

fn fmt(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 1.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{:.0} µs", ms * 1000.0)
    }
}

impl Report {
    pub fn new(filter: Option<String>) -> Self {
        Self { rows: Vec::new(), filter }
    }

    pub fn record(&mut self, name: &str, median: f64, p95: f64, budget: f64) {
        if self.filter.as_ref().is_some_and(|f| !name.to_lowercase().contains(&f.to_lowercase())) {
            return;
        }
        println!("  {name:<58} {:>9}", fmt(median));
        self.rows.push(Row { name: name.to_owned(), median, p95, budget });
    }

    /// Results as JSON, for the website. `meta` are extra top-level facts, already encoded.
    pub fn to_json(&self, meta: &[(&str, String)]) -> String {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let meta_json: Vec<String> = meta.iter().map(|(k, v)| format!("\"{k}\": {v}")).collect();
        let rows: Vec<String> = self
            .rows
            .iter()
            .map(|r| {
                format!(
                    "    {{\"name\": \"{}\", \"median_ms\": {:.4}, \"p95_ms\": {:.4}, \"budget_ms\": {:.4}}}",
                    esc(&r.name), r.median, r.p95, r.budget
                )
            })
            .collect();
        format!("{{\n  \"meta\": {{{}}},\n  \"results\": [\n{}\n  ]\n}}\n", meta_json.join(", "), rows.join(",\n"))
    }

    /// Print the summary table; returns how many budgets were missed.
    pub fn finish(&self) -> usize {
        println!("\n{:<60} {:>9} {:>9} {:>9}  status", "benchmark", "median", "p95", "budget");
        println!("{}", "-".repeat(104));
        let mut missed = 0;
        for r in &self.rows {
            let status = if r.median <= r.budget {
                "ok"
            } else if r.median <= r.budget * 2.0 {
                missed += 1;
                "SLOW (within 2x)"
            } else {
                missed += 1;
                "TOO SLOW"
            };
            println!("{:<60} {:>9} {:>9} {:>9}  {status}", r.name, fmt(r.median), fmt(r.p95), fmt(r.budget));
        }
        println!("{}", "-".repeat(104));
        println!("{} benchmarks, {} over budget", self.rows.len(), missed);
        missed
    }
}
