use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Fast incremental NTFS change detection
#[derive(Parser)]
#[command(name = "arachne-incremental")]
#[command(about = "Fast incremental file change detection for Windows NTFS")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create initial bootstrap state for a volume
    Bootstrap {
        /// Root path to scan
        #[arg(short, long)]
        path: PathBuf,
        /// Output bootstrap file
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Run incremental scan and output changes
    Scan {
        /// Bootstrap state file
        #[arg(short, long)]
        bootstrap: PathBuf,
        /// Baseline cache file (created on first run)
        #[arg(short, long)]
        baseline: PathBuf,
        /// Output file for events (JSONL format)
        #[arg(short, long)]
        output: PathBuf,
        /// Include only these operations (comma-separated: created,modified,renamed,deleted)
        #[arg(short, long, value_delimiter = ',')]
        ops: Option<Vec<String>>,
        /// Include only paths matching these patterns (comma-separated globs)
        #[arg(short, long, value_delimiter = ',')]
        include: Option<Vec<String>>,
        /// Exclude paths matching these patterns (comma-separated globs)
        #[arg(short, long, value_delimiter = ',')]
        exclude: Option<Vec<String>>,
        /// Output summary only (no individual events)
        #[arg(long)]
        summary: bool,
    },
    /// Watch for changes continuously
    Watch {
        /// Bootstrap state file
        #[arg(short, long)]
        bootstrap: PathBuf,
        /// Baseline cache file
        #[arg(short, long)]
        baseline: PathBuf,
        /// Poll interval in seconds
        #[arg(short, long, default_value = "5")]
        interval: u64,
        /// Include only these operations
        #[arg(short, long, value_delimiter = ',')]
        ops: Option<Vec<String>>,
        /// Include only paths matching these patterns (comma-separated globs)
        #[arg(short, long, value_delimiter = ',')]
        include: Option<Vec<String>>,
        /// Exclude paths matching these patterns
        #[arg(short, long, value_delimiter = ',')]
        exclude: Option<Vec<String>>,
        /// Output format (jsonl, log)
        #[arg(long, default_value = "log")]
        format: String,
    },
    /// Benchmark full scan vs incremental
    Benchmark {
        /// Test directory path
        #[arg(short, long)]
        path: PathBuf,
        /// Number of test files to create
        #[arg(short, long, default_value = "10000")]
        count: usize,
    },
}

/// Bootstrap state - persisted journal cursor
#[derive(Serialize, Deserialize, Debug, Clone)]
struct BootstrapState {
    journal_id: u64,
    next_usn: i64,
    volume_path: String,
    scan_root: String,
}

/// File entry in baseline cache
#[derive(Serialize, Deserialize, Debug, Clone)]
struct BaselineEntry {
    path: String,
    parent_frn: u64,
    exists: bool,
}

/// Cached baseline - FRN to entry mapping
#[derive(Serialize, Deserialize, Debug, Default)]
struct CachedBaseline {
    entries: HashMap<u64, BaselineEntry>,
    last_usn: i64,
}

/// File change event
#[derive(Serialize, Deserialize, Debug, Clone)]
struct FileEvent {
    op: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    frn: u64,
    usn: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<String>,
}

/// Scan summary statistics
#[derive(Serialize, Deserialize, Debug, Default)]
struct ScanSummary {
    total_events: usize,
    created: usize,
    modified: usize,
    renamed: usize,
    deleted: usize,
    scan_time_ms: u64,
    files_per_second: f64,
}

/// USN Record from journal
#[derive(Debug, Clone)]
struct UsnRecord {
    frn: u64,
    parent_frn: u64,
    usn: i64,
    reason: u32,
    filename: String,
}

/// Event filter configuration
struct EventFilter {
    ops: Option<Vec<String>>,
    include_patterns: Option<Vec<glob::Pattern>>,
    exclude_patterns: Option<Vec<glob::Pattern>>,
}

impl EventFilter {
    fn new(
        ops: Option<Vec<String>>,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
    ) -> Result<Self> {
        let include_patterns = include
            .map(|patterns| {
                patterns
                    .into_iter()
                    .map(|p| {
                        glob::Pattern::new(&p)
                            .with_context(|| format!("Invalid include glob pattern: {}", p))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;

        let exclude_patterns = exclude
            .map(|patterns| {
                patterns
                    .into_iter()
                    .map(|p| {
                        glob::Pattern::new(&p)
                            .with_context(|| format!("Invalid exclude glob pattern: {}", p))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;

        Ok(Self {
            ops,
            include_patterns,
            exclude_patterns,
        })
    }

    fn matches(&self, event: &FileEvent) -> bool {
        // Check operation filter
        if let Some(ref ops) = self.ops {
            if !ops.iter().any(|op| op.eq_ignore_ascii_case(&event.op)) {
                return false;
            }
        }

        // Check exclude patterns
        if let Some(ref excludes) = self.exclude_patterns {
            for pattern in excludes {
                if pattern.matches(&event.path) || pattern.matches(&event.filename()) {
                    return false;
                }
            }
        }

        // Check include patterns
        if let Some(ref includes) = self.include_patterns {
            let matches_include = includes
                .iter()
                .any(|pattern| pattern.matches(&event.path) || pattern.matches(&event.filename()));
            if !matches_include {
                return false;
            }
        }

        true
    }
}

impl FileEvent {
    fn filename(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Bootstrap { path, output } => cmd_bootstrap(path, output),
        Commands::Scan {
            bootstrap,
            baseline,
            output,
            ops,
            include,
            exclude,
            summary,
        } => cmd_scan(bootstrap, baseline, output, ops, include, exclude, summary),
        Commands::Watch {
            bootstrap,
            baseline,
            interval,
            ops,
            include,
            exclude,
            format,
        } => cmd_watch(bootstrap, baseline, interval, ops, include, exclude, format),
        Commands::Benchmark { path, count } => cmd_benchmark(path, count),
    }
}

fn now_iso() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f%z")
        .to_string()
}

/// Create bootstrap state by querying USN journal
fn cmd_bootstrap(path: PathBuf, output: PathBuf) -> Result<()> {
    let abs_path = std::fs::canonicalize(&path)
        .with_context(|| format!("Cannot access path: {}", path.display()))?;

    let volume = get_volume_from_path(&abs_path)?;

    println!("Creating bootstrap for: {}", abs_path.display());
    println!("Volume: {}", volume);

    let (journal_id, next_usn) = query_usn_journal(&volume)?;

    let bootstrap = BootstrapState {
        journal_id,
        next_usn,
        volume_path: volume,
        scan_root: abs_path.to_string_lossy().to_string(),
    };

    let json = serde_json::to_string_pretty(&bootstrap)?;
    std::fs::write(&output, json)
        .with_context(|| format!("Failed to write {}", output.display()))?;

    println!("Bootstrap created: {}", output.display());
    println!("  Journal ID: {:#018x}", bootstrap.journal_id);
    println!("  Next USN: {}", bootstrap.next_usn);

    Ok(())
}

/// Run incremental scan
fn cmd_scan(
    bootstrap_path: PathBuf,
    baseline_path: PathBuf,
    output_path: PathBuf,
    ops_filter: Option<Vec<String>>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    summary_only: bool,
) -> Result<()> {
    let start = Instant::now();

    let bootstrap: BootstrapState =
        serde_json::from_str(&std::fs::read_to_string(&bootstrap_path).with_context(|| {
            format!("Cannot read bootstrap file: {}", bootstrap_path.display())
        })?)
        .with_context(|| format!("Invalid bootstrap file: {}", bootstrap_path.display()))?;

    let filter = EventFilter::new(ops_filter, include, exclude)?;

    let mut baseline: CachedBaseline = if baseline_path.exists() {
        let raw = std::fs::read_to_string(&baseline_path)
            .with_context(|| format!("Cannot read baseline file: {}", baseline_path.display()))?;
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "Baseline file is corrupt: {}. Delete it to start fresh.",
                baseline_path.display()
            )
        })?
    } else {
        CachedBaseline::default()
    };

    // Verify the journal has not been recreated since bootstrap.
    let (live_journal_id, _) = query_usn_journal(&bootstrap.volume_path)?;
    if live_journal_id != bootstrap.journal_id {
        return Err(anyhow::anyhow!(
            "USN journal ID changed ({:#018x} → {:#018x}); delete the baseline and re-run bootstrap",
            bootstrap.journal_id,
            live_journal_id
        ));
    }

    let (records, last_next_usn) = read_usn_deltas(
        &bootstrap.volume_path,
        baseline.last_usn,
        bootstrap.journal_id,
    )?;
    let read_time = start.elapsed();

    let events = process_records(records, &mut baseline, &bootstrap.scan_root)?;

    // Filter events
    let filtered_events: Vec<FileEvent> = events
        .into_iter()
        .filter(|e| filter.matches(e))
        .map(|mut e| {
            e.timestamp = Some(now_iso());
            e
        })
        .collect();

    let scan_time_ms = read_time.as_millis() as u64;

    if summary_only {
        let summary = create_summary(&filtered_events, scan_time_ms);
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        let mut output = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&output_path)?;

        for event in &filtered_events {
            writeln!(output, "{}", serde_json::to_string(event)?)?;
        }

        println!(
            "Detected {} changes in {}ms",
            filtered_events.len(),
            scan_time_ms
        );
        println!("Output: {}", output_path.display());
    }

    // Always advance the cursor to the last USN read from the journal,
    // regardless of how many events passed the filter.
    baseline.last_usn = last_next_usn;

    std::fs::write(&baseline_path, serde_json::to_string_pretty(&baseline)?)?;

    Ok(())
}

/// Watch mode - continuous monitoring
fn cmd_watch(
    bootstrap_path: PathBuf,
    baseline_path: PathBuf,
    interval_secs: u64,
    ops_filter: Option<Vec<String>>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    format: String,
) -> Result<()> {
    let bootstrap: BootstrapState =
        serde_json::from_str(&std::fs::read_to_string(&bootstrap_path).with_context(|| {
            format!("Cannot read bootstrap file: {}", bootstrap_path.display())
        })?)
        .with_context(|| format!("Invalid bootstrap file: {}", bootstrap_path.display()))?;

    let filter = EventFilter::new(ops_filter, include, exclude)?;

    let mut baseline: CachedBaseline = if baseline_path.exists() {
        let raw = std::fs::read_to_string(&baseline_path)
            .with_context(|| format!("Cannot read baseline file: {}", baseline_path.display()))?;
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "Baseline file is corrupt: {}. Delete it to start fresh.",
                baseline_path.display()
            )
        })?
    } else {
        CachedBaseline::default()
    };

    // Verify the journal has not been recreated since bootstrap.
    let (live_journal_id, _) = query_usn_journal(&bootstrap.volume_path)?;
    if live_journal_id != bootstrap.journal_id {
        return Err(anyhow::anyhow!(
            "USN journal ID changed ({:#018x} → {:#018x}); delete the baseline and re-run bootstrap",
            bootstrap.journal_id,
            live_journal_id
        ));
    }

    println!("Watching: {}", bootstrap.scan_root);
    println!("Poll interval: {}s", interval_secs);
    println!("Press Ctrl+C to stop\n");

    let interval = Duration::from_secs(interval_secs);

    loop {
        let start = Instant::now();

        match read_usn_deltas(
            &bootstrap.volume_path,
            baseline.last_usn,
            bootstrap.journal_id,
        ) {
            Ok((records, last_next_usn)) => {
                let events = process_records(records, &mut baseline, &bootstrap.scan_root)?;

                let filtered_events: Vec<FileEvent> = events
                    .into_iter()
                    .filter(|e| filter.matches(e))
                    .map(|mut e| {
                        e.timestamp = Some(now_iso());
                        e
                    })
                    .collect();

                if !filtered_events.is_empty() {
                    for event in &filtered_events {
                        match format.as_str() {
                            "jsonl" => println!("{}", serde_json::to_string(event)?),
                            _ => println!(
                                "[{}] {}: {}",
                                event.timestamp.as_ref().unwrap_or(&"?".to_string()),
                                event.op.to_uppercase(),
                                event.path
                            ),
                        }
                    }
                }

                // Always advance the cursor to the last USN read from the journal.
                baseline.last_usn = last_next_usn;
                let _ = std::fs::write(&baseline_path, serde_json::to_string_pretty(&baseline)?);
            }
            Err(e) => {
                eprintln!("[ERROR] Failed to read USN journal: {}", e);
            }
        }

        let elapsed = start.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }
}

fn create_summary(events: &[FileEvent], scan_time_ms: u64) -> ScanSummary {
    let mut summary = ScanSummary {
        total_events: events.len(),
        scan_time_ms,
        ..Default::default()
    };

    for event in events {
        match event.op.as_str() {
            "created" => summary.created += 1,
            "modified" => summary.modified += 1,
            "renamed" => summary.renamed += 1,
            "deleted" => summary.deleted += 1,
            _ => {}
        }
    }

    summary.files_per_second = if scan_time_ms > 0 {
        (events.len() as f64 / scan_time_ms as f64) * 1000.0
    } else {
        0.0
    };

    summary
}

/// Benchmark full scan vs incremental
fn cmd_benchmark(path: PathBuf, count: usize) -> Result<()> {
    let test_dir = path.join("benchmark_test");

    println!("Arachne Incremental Benchmark");
    println!("==============================");
    println!("Test files: {}", count);
    println!();

    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir)?;

    println!("[1/5] Creating {} test files...", count);
    let start = Instant::now();
    for i in 0..count {
        let file_path = test_dir.join(format!("file_{}.txt", i));
        std::fs::write(&file_path, format!("content {}", i))?;
    }
    let create_time = start.elapsed();
    println!("      Created in {:.2}s", create_time.as_secs_f64());

    println!("\n[2/5] FULL scan (directory walk)...");
    let start = Instant::now();
    let mut full_scan_count = 0;
    walk_directory(&test_dir, &mut full_scan_count)?;
    let full_scan_time = start.elapsed();
    println!("      Files scanned: {}", full_scan_count);
    println!("      Time: {:.2}s", full_scan_time.as_secs_f64());

    println!("\n[3/5] Creating bootstrap (USN journal cursor)...");
    let bootstrap_path = test_dir.join("bootstrap.json");
    cmd_bootstrap(test_dir.clone(), bootstrap_path.clone())?;

    println!("      Creating baseline cache...");
    let baseline_path = test_dir.join("baseline.json");
    cmd_scan(
        bootstrap_path.clone(),
        baseline_path.clone(),
        test_dir.join("events_init.jsonl"),
        None,
        None,
        None,
        false,
    )?;

    let modify_count = count.max(100) / 100;
    println!("\n[4/5] Modifying {} files (1% churn)...", modify_count);
    for i in 0..modify_count {
        let file_path = test_dir.join(format!("file_{}.txt", i));
        std::fs::write(&file_path, format!("modified content {}", i))?;
    }
    println!("      Done");

    println!("\n[5/5] INCREMENTAL scan (USN journal)...");

    let bootstrap: BootstrapState =
        serde_json::from_str(&std::fs::read_to_string(&bootstrap_path)?)?;

    std::thread::sleep(std::time::Duration::from_millis(100));

    let start = Instant::now();
    let (records, _) = read_usn_deltas(
        &bootstrap.volume_path,
        bootstrap.next_usn,
        bootstrap.journal_id,
    )?;
    let incremental_time = start.elapsed();

    let relevant_records: Vec<_> = records
        .iter()
        .filter(|r| r.filename.contains("file_") && r.filename.contains(".txt"))
        .collect();

    println!("      Total USN records read: {}", records.len());
    println!("      Test directory records: {}", relevant_records.len());
    println!(
        "      Time: {:.1}ms",
        incremental_time.as_secs_f64() * 1000.0
    );

    let events_output = test_dir.join("events_detected.jsonl");
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&events_output)?;

    for record in &relevant_records {
        let event = serde_json::json!({
            "op": classify_reason(record.reason),
            "filename": record.filename,
            "frn": record.frn,
            "usn": record.usn,
            "reason": format!("0x{:08x}", record.reason),
        });
        writeln!(output, "{}", event)?;
    }

    let new_events = relevant_records.len();

    println!("\n==============================");
    println!("RESULTS");
    println!("==============================");
    println!();
    println!(
        "Full Directory Walk:  {:.2}s (scanned {} files)",
        full_scan_time.as_secs_f64(),
        full_scan_count
    );
    println!(
        "USN Journal Scan:     {:.1}ms (read {} deltas)",
        incremental_time.as_secs_f64() * 1000.0,
        new_events
    );

    let reduction = if full_scan_time.as_secs_f64() > 0.0 {
        (1.0 - (incremental_time.as_secs_f64() / full_scan_time.as_secs_f64())) * 100.0
    } else {
        0.0
    };

    println!("Performance Gain:     {:.1}% faster", reduction);
    println!();

    if reduction >= 80.0 {
        println!("✅ PASS: >= 80% latency reduction achieved!");
    } else {
        println!("⚠️  Result: {:.1}% reduction", reduction);
    }

    if let Ok(events) = std::fs::read_to_string(&events_output) {
        let lines: Vec<&str> = events.lines().collect();
        if !lines.is_empty() {
            println!("\nSample detected changes:");
            for (i, line) in lines.iter().take(5).enumerate() {
                println!("  {}: {}", i + 1, line);
            }
            if lines.len() > 5 {
                println!("  ... and {} more", lines.len() - 5);
            }
        } else {
            println!("\n⚠️  No changes detected in USN journal");
            println!("   This can happen if:");
            println!("   - The USN journal doesn't cover the file modifications");
            println!("   - The journal was truncated between operations");
        }
    }

    println!("\n==============================");
    println!("Test folder kept for inspection:");
    println!("  {}", test_dir.display());
    println!(
        "To cleanup: Remove-Item -Recurse -Force '{}'",
        test_dir.display()
    );

    Ok(())
}

fn walk_directory(dir: &Path, count: &mut usize) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        *count += 1;
        if path.is_dir() {
            walk_directory(&path, count)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn query_usn_journal(volume: &str) -> Result<(u64, i64)> {
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;
    use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ};

    const FSCTL_QUERY_USN_JOURNAL: u32 = 0x000900f4;

    #[repr(C)]
    struct UsnJournalData {
        usn_journal_id: u64,
        first_usn: i64,
        next_usn: i64,
        lowest_valid_usn: i64,
        max_usn: i64,
        maximum_size: u64,
        allocation_delta: u64,
    }

    let volume_path: Vec<u16> = format!("\\\\.\\{}:", volume.trim_end_matches(':'))
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let handle = CreateFileW(
            volume_path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );

        if handle == INVALID_HANDLE_VALUE {
            return Err(anyhow::anyhow!("Failed to open volume {}", volume));
        }

        let mut journal_data: UsnJournalData = std::mem::zeroed();
        let mut bytes_returned: u32 = 0;

        let result = DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null_mut(),
            0,
            &mut journal_data as *mut _ as *mut _,
            std::mem::size_of::<UsnJournalData>() as u32,
            &mut bytes_returned,
            std::ptr::null_mut(),
        );

        winapi::um::handleapi::CloseHandle(handle);

        if result == 0 {
            return Err(anyhow::anyhow!(
                "USN journal query failed - journal may not exist. Run: fsutil usn createjournal {}:",
                volume
            ));
        }

        Ok((journal_data.usn_journal_id, journal_data.next_usn))
    }
}

#[cfg(windows)]
fn read_usn_deltas(volume: &str, start_usn: i64, journal_id: u64) -> Result<(Vec<UsnRecord>, i64)> {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;
    use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ};

    // https://docs.microsoft.com/windows/win32/api/winioctl/ni-winioctl-fsctl_read_usn_journal
    const FSCTL_READ_USN_JOURNAL: u32 = 0x000900bb;
    // Win32 error codes relevant to USN journal reads.
    const ERROR_HANDLE_EOF: u32 = 38;
    const ERROR_JOURNAL_ENTRY_DELETED: u32 = 1181; // 0x49D
    const ERROR_JOURNAL_NOT_ACTIVE: u32 = 1179; // 0x49B
                                                // Buffer large enough for many records per call (512 KB).
    const BUFFER_SIZE: usize = 512 * 1024;

    #[repr(C)]
    struct ReadUsnJournalData {
        start_usn: i64,
        reason_mask: u32,
        return_only_on_close: u32,
        timeout: u64,
        bytes_to_wait_for: u64,
        usn_journal_id: u64,
    }

    // The record layout defined by the Windows SDK. Declared packed so that
    // the compiler does not add padding between fields. All field accesses
    // must use `ptr::read_unaligned` to avoid undefined behaviour.
    #[repr(C, packed)]
    struct UsnRecordV2 {
        record_length: u32,
        major_version: u16,
        minor_version: u16,
        file_reference_number: u64,
        parent_file_reference_number: u64,
        usn: i64,
        time_stamp: i64,
        reason: u32,
        source_info: u32,
        security_id: u32,
        file_attributes: u32,
        file_name_length: u16,
        file_name_offset: u16,
    }

    let volume_path: Vec<u16> = format!("\\\\.\\{}:", volume.trim_end_matches(':'))
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // Open the volume handle once; it stays open for the entire read loop.
    let handle = unsafe {
        CreateFileW(
            volume_path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return Err(anyhow::anyhow!("Failed to open volume {}", volume));
    }

    // RAII guard to ensure the handle is always closed.
    struct HandleGuard(winapi::um::winnt::HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let _guard = HandleGuard(handle);

    let mut all_records: Vec<UsnRecord> = Vec::new();
    let mut current_usn = start_usn;
    let mut last_next_usn = start_usn;
    let mut buffer: Vec<u8> = vec![0u8; BUFFER_SIZE];

    loop {
        let read_data = ReadUsnJournalData {
            start_usn: current_usn,
            reason_mask: 0xFFFF_FFFF,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            usn_journal_id: journal_id,
        };

        let mut bytes_returned: u32 = 0;

        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                &read_data as *const _ as *mut _,
                std::mem::size_of::<ReadUsnJournalData>() as u32,
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut bytes_returned,
                std::ptr::null_mut(),
            )
        };

        if ok == 0 {
            let err = unsafe { GetLastError() };
            match err {
                ERROR_HANDLE_EOF => break, // no more records
                ERROR_JOURNAL_ENTRY_DELETED => {
                    return Err(anyhow::anyhow!(
                        "USN journal entries have been deleted (journal wrapped); \
                         delete the baseline and re-run bootstrap"
                    ));
                }
                ERROR_JOURNAL_NOT_ACTIVE => {
                    return Err(anyhow::anyhow!(
                        "USN journal is not active on volume {}; \
                         enable it with: fsutil usn createjournal m=1000 a=100 {}:",
                        volume,
                        volume
                    ));
                }
                _ => break, // unexpected error - stop reading rather than looping
            }
        }

        // The first 8 bytes of the output buffer are always the next USN to
        // continue reading from. If no records follow (bytes_returned == 8),
        // we have drained the journal.
        let header_size = std::mem::size_of::<i64>();
        if (bytes_returned as usize) <= header_size {
            last_next_usn = i64::from_ne_bytes(
                buffer[..header_size]
                    .try_into()
                    .expect("buffer is at least 8 bytes"),
            );
            break;
        }

        last_next_usn = i64::from_ne_bytes(
            buffer[..header_size]
                .try_into()
                .expect("buffer is at least 8 bytes"),
        );
        current_usn = last_next_usn;

        // Parse USN records that follow the 8-byte header.
        let mut offset = header_size;
        let mut records_in_batch = 0usize;

        while offset + std::mem::size_of::<UsnRecordV2>() <= bytes_returned as usize {
            // Safety: `offset` is within `bytes_returned` bytes of a valid Vec<u8>.
            // We use `read_unaligned` for every field because the struct is packed.
            let record_ptr = unsafe { buffer.as_ptr().add(offset) as *const UsnRecordV2 };

            let record_length = unsafe {
                std::ptr::read_unaligned(std::ptr::addr_of!((*record_ptr).record_length))
            };

            if record_length == 0 {
                break;
            }
            if offset + record_length as usize > bytes_returned as usize {
                break;
            }

            let file_name_length = unsafe {
                std::ptr::read_unaligned(std::ptr::addr_of!((*record_ptr).file_name_length))
            } as usize;
            let file_name_offset = unsafe {
                std::ptr::read_unaligned(std::ptr::addr_of!((*record_ptr).file_name_offset))
            } as usize;

            // `file_name_offset` is relative to the start of the record, not the buffer.
            let name_buf_offset = offset + file_name_offset;

            if file_name_length > 0 && name_buf_offset + file_name_length <= bytes_returned as usize
            {
                let frn = unsafe {
                    std::ptr::read_unaligned(std::ptr::addr_of!(
                        (*record_ptr).file_reference_number
                    ))
                };
                let parent_frn = unsafe {
                    std::ptr::read_unaligned(std::ptr::addr_of!(
                        (*record_ptr).parent_file_reference_number
                    ))
                };
                let usn =
                    unsafe { std::ptr::read_unaligned(std::ptr::addr_of!((*record_ptr).usn)) };
                let reason =
                    unsafe { std::ptr::read_unaligned(std::ptr::addr_of!((*record_ptr).reason)) };

                // Safety: `name_buf_offset` and `file_name_length` are within the buffer.
                let name_slice = unsafe {
                    std::slice::from_raw_parts(
                        buffer.as_ptr().add(name_buf_offset) as *const u16,
                        file_name_length / 2,
                    )
                };
                let filename = String::from_utf16_lossy(name_slice).into();

                all_records.push(UsnRecord {
                    frn,
                    parent_frn,
                    usn,
                    reason,
                    filename,
                });
                records_in_batch += 1;
            }

            offset += record_length as usize;
        }

        if records_in_batch == 0 {
            break;
        }
    }

    Ok((all_records, last_next_usn))
}

#[cfg(not(windows))]
fn query_usn_journal(_volume: &str) -> Result<(u64, i64)> {
    Ok((0x123456789abcdef0, 1000000))
}

#[cfg(not(windows))]
fn read_usn_deltas(
    _volume: &str,
    start_usn: i64,
    _journal_id: u64,
) -> Result<(Vec<UsnRecord>, i64)> {
    Ok((vec![], start_usn))
}

fn process_records(
    records: Vec<UsnRecord>,
    baseline: &mut CachedBaseline,
    scan_root: &str,
) -> Result<Vec<FileEvent>> {
    let mut events = Vec::new();
    let mut seen_frns = std::collections::HashSet::new();

    for record in records {
        if !seen_frns.insert(record.frn) {
            continue;
        }

        let op = classify_reason(record.reason);
        let filename = sanitize_filename(&record.filename);
        let path = if let Some(parent) = baseline.entries.get(&record.parent_frn) {
            format!("{}/{}", parent.path, filename)
        } else {
            format!("{}/{}", scan_root, filename)
        };

        if path.starts_with(scan_root) {
            let old_path = if op == "renamed" {
                baseline.entries.get(&record.frn).map(|e| e.path.clone())
            } else {
                None
            };

            events.push(FileEvent {
                op: op.to_string(),
                path: path.clone(),
                old_path,
                frn: record.frn,
                usn: record.usn,
                timestamp: None,
            });

            baseline.entries.insert(
                record.frn,
                BaselineEntry {
                    path,
                    parent_frn: record.parent_frn,
                    exists: op != "deleted",
                },
            );
        }
    }

    Ok(events)
}

fn classify_reason(reason: u32) -> &'static str {
    const FILE_CREATE: u32 = 0x00000100;
    const FILE_DELETE: u32 = 0x00000200;
    const RENAME_OLD: u32 = 0x00001000;
    const RENAME_NEW: u32 = 0x00002000;

    if reason & FILE_DELETE != 0 {
        "deleted"
    } else if (reason & (RENAME_OLD | RENAME_NEW)) == (RENAME_OLD | RENAME_NEW) {
        "renamed"
    } else if reason & RENAME_OLD != 0 {
        "renamed_old"
    } else if reason & RENAME_NEW != 0 {
        "renamed_new"
    } else if reason & FILE_CREATE != 0 {
        "created"
    } else {
        "modified"
    }
}

fn sanitize_filename(name: &str) -> String {
    name.replace(['\0'], "")
}

fn get_volume_from_path(path: &Path) -> Result<String> {
    let canonical = std::fs::canonicalize(path)?;
    let path_str = canonical.to_string_lossy();

    let drive_letter = if path_str.starts_with(r"\\?\") {
        path_str.chars().nth(4)
    } else {
        path_str.chars().next()
    }
    .ok_or_else(|| anyhow::anyhow!("Invalid path: {}", path_str))?;

    Ok(drive_letter.to_uppercase().to_string())
}
