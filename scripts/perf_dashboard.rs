use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use lkr_core::{
    perf::scenarios::{PreparedScriptScenario, prepare_script_scenarios},
    vm::Vm,
};
use serde::{Deserialize, Serialize};
use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const DASHBOARD_TEMPLATE: &str = include_str!("perf_dashboard_template.html");

struct TrackingAllocator;

#[derive(Default)]
struct AllocTracker {
    current: AtomicUsize,
    peak: AtomicUsize,
    baseline: AtomicUsize,
}

static ALLOC_TRACKER: AllocTracker = AllocTracker {
    current: AtomicUsize::new(0),
    peak: AtomicUsize::new(0),
    baseline: AtomicUsize::new(0),
};

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        record_dealloc(layout.size());
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, old_layout, new_size) };
        if !new_ptr.is_null() {
            adjust_realloc(old_layout.size(), new_size);
        }
        new_ptr
    }
}

fn record_alloc(size: usize) {
    let current = ALLOC_TRACKER
        .current
        .fetch_add(size, Ordering::SeqCst)
        .saturating_add(size);
    ALLOC_TRACKER.peak.fetch_max(current, Ordering::SeqCst);
}

fn record_dealloc(size: usize) {
    ALLOC_TRACKER.current.fetch_sub(size, Ordering::SeqCst);
}

fn adjust_realloc(old: usize, new: usize) {
    if new > old {
        record_alloc(new - old);
    } else if old > new {
        record_dealloc(old - new);
    }
}

fn reset_peak() {
    let current = ALLOC_TRACKER.current.load(Ordering::SeqCst);
    ALLOC_TRACKER.baseline.store(current, Ordering::SeqCst);
    ALLOC_TRACKER.peak.store(current, Ordering::SeqCst);
}

fn peak_since_reset() -> usize {
    let peak = ALLOC_TRACKER.peak.load(Ordering::SeqCst);
    let baseline = ALLOC_TRACKER.baseline.load(Ordering::SeqCst);
    peak.saturating_sub(baseline)
}

#[derive(Debug)]
struct Options {
    run_bench: bool,
    benches: Vec<String>,
    criterion_dir: PathBuf,
    output_dir: PathBuf,
    history_limit: usize,
    memory_iterations: usize,
    timestamp: DateTime<Utc>,
    notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ScenarioMetrics {
    scenario: String,
    scenario_title: String,
    variant: String,
    mean_ns: f64,
    median_ns: f64,
    std_dev_ns: f64,
    p50_ns: f64,
    p95_ns: f64,
    p99_ns: f64,
    memory_peak_bytes: u64,
    #[serde(default)]
    region_heap_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunRecord {
    generated_at: String,
    git_rev: Option<String>,
    notes: Option<String>,
    metrics: Vec<ScenarioMetrics>,
}

#[derive(Debug, Serialize)]
struct HistoryFile {
    generated_at: String,
    history_limit: usize,
    runs: Vec<RunRecord>,
}

#[derive(Deserialize)]
struct EstimateFile {
    mean: EstimateEntry,
    median: EstimateEntry,
    std_dev: EstimateEntry,
}

#[derive(Deserialize)]
struct EstimateEntry {
    point_estimate: f64,
}

struct TimeStats {
    mean_ns: f64,
    median_ns: f64,
    std_dev_ns: f64,
    p50_ns: f64,
    p95_ns: f64,
    p99_ns: f64,
}

struct MemoryMetrics {
    peak_bytes: usize,
    vm_heap_bytes: u64,
}

fn main() -> Result<()> {
    let opts = parse_args(env::args().skip(1))?;
    if opts.run_bench {
        for bench in &opts.benches {
            run_cargo_bench(bench)?;
        }
    }

    if !opts.output_dir.exists() {
        fs::create_dir_all(&opts.output_dir)
            .with_context(|| format!("create output directory {}", opts.output_dir.display()))?;
    }
    let history_dir = opts.output_dir.join("history");
    if !history_dir.exists() {
        fs::create_dir_all(&history_dir)
            .with_context(|| format!("create history directory {}", history_dir.display()))?;
    }

    let prepared = prepare_script_scenarios().context("prepare benchmark scenarios")?;
    let mut run_metrics = Vec::new();

    for scenario in &prepared {
        let case_name = scenario.bench_case_name();
        let time_stats = load_time_stats(&opts.criterion_dir, &case_name)?;
        let memory_stats = measure_peak_memory(scenario, opts.memory_iterations)?;
        run_metrics.push(ScenarioMetrics {
            scenario: scenario.key().to_string(),
            scenario_title: scenario.title().to_string(),
            variant: "vm".to_string(),
            mean_ns: time_stats.mean_ns,
            median_ns: time_stats.median_ns,
            std_dev_ns: time_stats.std_dev_ns,
            p50_ns: time_stats.p50_ns,
            p95_ns: time_stats.p95_ns,
            p99_ns: time_stats.p99_ns,
            memory_peak_bytes: memory_stats.peak_bytes as u64,
            region_heap_bytes: memory_stats.vm_heap_bytes,
        });
    }

    let git_rev = env::var("GITHUB_SHA")
        .ok()
        .map(|sha| sha.chars().take(8).collect::<String>());
    let timestamp_iso = opts.timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let run_record = RunRecord {
        generated_at: timestamp_iso.clone(),
        git_rev,
        notes: opts.notes.clone(),
        metrics: run_metrics,
    };

    let filename_stamp = opts.timestamp.format("%Y%m%dT%H%M%SZ").to_string();
    write_json(&opts.output_dir.join("latest.json"), &run_record)?;
    write_csv(&opts.output_dir.join("latest.csv"), &run_record.metrics)?;
    write_json(&history_dir.join(format!("{}.json", filename_stamp)), &run_record)?;
    write_csv(
        &history_dir.join(format!("{}.csv", filename_stamp)),
        &run_record.metrics,
    )?;

    prune_history(&history_dir, opts.history_limit)?;
    let history_runs = load_history_runs(&history_dir, opts.history_limit)?;
    let timeline = HistoryFile {
        generated_at: timestamp_iso,
        history_limit: opts.history_limit,
        runs: history_runs,
    };
    let history_json_path = opts.output_dir.join("history.json");
    write_json(&history_json_path, &timeline)?;

    let dashboard_html = render_dashboard(&timeline)?;
    let index_html_path = opts.output_dir.join("index.html");
    fs::write(&index_html_path, dashboard_html.as_bytes())
        .with_context(|| format!("write {}", index_html_path.display()))?;

    println!(
        "Performance dashboard updated -> {}, {}, {}",
        opts.output_dir.join("latest.json").display(),
        history_json_path.display(),
        index_html_path.display()
    );
    Ok(())
}

fn parse_args<I>(args: I) -> Result<Options>
where
    I: IntoIterator<Item = String>,
{
    let mut opts = Options {
        run_bench: true,
        benches: vec!["scripts_bench".to_string()],
        criterion_dir: PathBuf::from("target/criterion"),
        output_dir: PathBuf::from("docs/perf/dashboard"),
        history_limit: 30,
        memory_iterations: 3,
        timestamp: Utc::now(),
        notes: None,
    };

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bench" => {
                let value = iter.next().context("expected <name> after --bench")?;
                opts.benches.push(value);
            }
            "--benches" => {
                let value = iter.next().context("expected comma separated values after --benches")?;
                opts.benches = value
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
            }
            "--criterion-dir" => {
                let value = iter.next().context("expected path after --criterion-dir <dir>")?;
                opts.criterion_dir = PathBuf::from(value);
            }
            "--output-dir" => {
                let value = iter.next().context("expected path after --output-dir <dir>")?;
                opts.output_dir = PathBuf::from(value);
            }
            "--history-limit" => {
                let value = iter.next().context("expected integer after --history-limit <n>")?;
                opts.history_limit = value.parse().context("parse --history-limit as positive integer")?;
            }
            "--memory-iters" => {
                let value = iter.next().context("expected integer after --memory-iters <n>")?;
                opts.memory_iterations = value.parse().context("parse --memory-iters as positive integer")?;
            }
            "--timestamp" => {
                let value = iter.next().context("expected RFC3339 timestamp after --timestamp")?;
                opts.timestamp = DateTime::parse_from_rfc3339(&value)
                    .context("parse --timestamp as RFC3339")?
                    .with_timezone(&Utc);
            }
            "--notes" => {
                let value = iter.next().context("expected string after --notes")?;
                opts.notes = Some(value);
            }
            "--skip-bench" => opts.run_bench = false,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                return Err(anyhow!(
                    "unknown argument '{}'. Use --help for usage information.",
                    other
                ));
            }
        }
    }

    if opts.benches.is_empty() {
        opts.benches.push("scripts_bench".to_string());
    }

    Ok(opts)
}

fn print_help() {
    println!("Usage: perf_dashboard [options]");
    println!();
    println!("Options:");
    println!("  --bench <name>           Append a bench target (can repeat)");
    println!("  --benches a,b,c          Replace bench list with comma-separated values");
    println!("  --criterion-dir <path>   Path to Criterion output (default: target/criterion)");
    println!("  --output-dir <path>      Destination directory for dashboard artifacts");
    println!("  --history-limit <n>      Max history snapshots to keep (default: 30)");
    println!("  --memory-iters <n>       Peak memory sampling iterations (default: 3)");
    println!("  --timestamp <RFC3339>    Override timestamp for this run");
    println!("  --notes <text>           Attach notes to this snapshot");
    println!("  --skip-bench             Skip running cargo bench; reuse existing output");
    println!("  --help, -h               Show this help message");
}

fn run_cargo_bench(bench: &str) -> Result<()> {
    let status = Command::new("cargo")
        .args(["bench", "-p", "lkr-core", "--bench", bench, "--", "--noplot"])
        .status()
        .context("failed to spawn cargo bench")?;
    if !status.success() {
        return Err(anyhow!("cargo bench --bench {} exited with {}", bench, status));
    }
    Ok(())
}

fn load_time_stats(criterion_dir: &Path, case: &str) -> Result<TimeStats> {
    let estimate_path = criterion_dir.join(case).join("new").join("estimates.json");
    let data = fs::read_to_string(&estimate_path).with_context(|| format!("read {}", estimate_path.display()))?;
    let estimates: EstimateFile =
        serde_json::from_str(&data).with_context(|| format!("parse {}", estimate_path.display()))?;

    let raw_path = criterion_dir.join(case).join("new").join("raw.csv");
    let raw_file = File::open(&raw_path).with_context(|| format!("open {}", raw_path.display()))?;
    let mut reader = BufReader::new(raw_file);
    let mut line = String::new();
    let mut samples = Vec::new();
    while reader.read_line(&mut line)? != 0 {
        if line.starts_with("group") {
            line.clear();
            continue;
        }
        let sample = parse_sample_value(&line)?;
        samples.push(sample);
        line.clear();
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = quantile(&samples, 0.5);
    let p95 = quantile(&samples, 0.95);
    let p99 = quantile(&samples, 0.99);

    Ok(TimeStats {
        mean_ns: estimates.mean.point_estimate,
        median_ns: estimates.median.point_estimate,
        std_dev_ns: estimates.std_dev.point_estimate,
        p50_ns: p50,
        p95_ns: p95,
        p99_ns: p99,
    })
}

fn parse_sample_value(line: &str) -> Result<f64> {
    let parts: Vec<&str> = line.trim_end().split(',').collect();
    if parts.len() < 8 {
        return Err(anyhow!("raw.csv row had {} columns, expected at least 8", parts.len()));
    }
    let raw_value: f64 = parts[5]
        .parse()
        .context("raw.csv contained a non-numeric sample_measured_value")?;
    let iterations: f64 = parts[7]
        .parse()
        .context("raw.csv contained a non-numeric iteration_count")?;
    if iterations > 0.0 {
        Ok(raw_value / iterations)
    } else {
        Ok(raw_value)
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = pos.floor() as usize;
    let upper = pos.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = pos - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

fn measure_peak_memory(scenario: &PreparedScriptScenario, iterations: usize) -> Result<MemoryMetrics> {
    let mut max_peak = 0usize;

    let mut vm = Vm::new();
    let warm = scenario.run_with_vm(&mut vm)?;
    let mut max_heap = warm.heap_bytes;
    drop(warm.value);

    for _ in 0..iterations.max(1) {
        reset_peak();
        let mut vm = Vm::new();
        let outcome = scenario.run_with_vm(&mut vm)?;
        drop(outcome.value);
        let peak = peak_since_reset();
        if peak > max_peak {
            max_peak = peak;
        }
        if outcome.heap_bytes > max_heap {
            max_heap = outcome.heap_bytes;
        }
    }

    Ok(MemoryMetrics {
        peak_bytes: max_peak,
        vm_heap_bytes: max_heap,
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), value).with_context(|| format!("write {}", path.display()))
}

fn write_csv(path: &Path, metrics: &[ScenarioMetrics]) -> Result<()> {
    let mut writer = BufWriter::new(File::create(path).with_context(|| format!("create {}", path.display()))?);
    writeln!(
        writer,
        "scenario,variant,mean_ns,median_ns,std_dev_ns,p50_ns,p95_ns,p99_ns,memory_peak_bytes,region_heap_bytes"
    )?;
    for metric in metrics {
        writeln!(
            writer,
            "{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{}",
            metric.scenario,
            metric.variant,
            metric.mean_ns,
            metric.median_ns,
            metric.std_dev_ns,
            metric.p50_ns,
            metric.p95_ns,
            metric.p99_ns,
            metric.memory_peak_bytes,
            metric.region_heap_bytes
        )?;
    }
    writer.flush()?;
    Ok(())
}

fn prune_history(history_dir: &Path, limit: usize) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(history_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().map(|ext| ext == "json").unwrap_or(false))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    while entries.len() > limit {
        let entry = entries.remove(0);
        let path = entry.path();
        fs::remove_file(&path).with_context(|| format!("remove old history snapshot {}", path.display()))?;
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let csv_path = history_dir.join(format!("{}.csv", stem));
            if csv_path.exists() {
                fs::remove_file(csv_path).ok();
            }
        }
    }
    Ok(())
}

fn load_history_runs(history_dir: &Path, limit: usize) -> Result<Vec<RunRecord>> {
    let mut entries: Vec<_> = fs::read_dir(history_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().map(|ext| ext == "json").unwrap_or(false))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    entries.reverse();
    entries.truncate(limit);

    let mut runs = Vec::new();
    for entry in entries {
        let path = entry.path();
        let data = fs::read_to_string(&path).with_context(|| format!("read history snapshot {}", path.display()))?;
        let mut record: RunRecord = serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))?;
        record
            .metrics
            .sort_by(|a, b| a.scenario.cmp(&b.scenario).then(a.variant.cmp(&b.variant)));
        runs.push(record);
    }
    Ok(runs)
}

fn render_dashboard(history: &HistoryFile) -> Result<String> {
    let json = serde_json::to_string(history).context("serialize history to embed in HTML")?;
    Ok(DASHBOARD_TEMPLATE.replace("__DATA_PLACEHOLDER__", &json))
}
