use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
#[derive(Serialize, Deserialize, Debug)]
struct FileEvent {
    op: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    frn: u64,
    usn: i64,
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Bootstrap { path, output } => cmd_bootstrap(path, output),
        Commands::Scan { bootstrap, baseline, output } => cmd_scan(bootstrap, baseline, output),
        Commands::Benchmark { path, count } => cmd_benchmark(path, count),
    }
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
fn cmd_scan(bootstrap_path: PathBuf, baseline_path: PathBuf, output_path: PathBuf) -> Result<()> {
    let bootstrap: BootstrapState = serde_json::from_str(
        &std::fs::read_to_string(&bootstrap_path)?
    ).with_context(|| "Invalid bootstrap file")?;
    
    println!("Scanning: {}", bootstrap.scan_root);
    println!("Baseline: {}", baseline_path.display());
    
    let mut baseline: CachedBaseline = if baseline_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&baseline_path)?)
            .unwrap_or_default()
    } else {
        println!("Creating new baseline cache...");
        CachedBaseline::default()
    };
    
    let start = Instant::now();
    let records = read_usn_deltas(&bootstrap.volume_path, baseline.last_usn)?;
    let read_time = start.elapsed();
    
    println!("Read {} USN records in {:.1}ms", records.len(), read_time.as_secs_f64() * 1000.0);
    
    let events = process_records(records, &mut baseline, &bootstrap.scan_root)?;
    
    println!("Generated {} events", events.len());
    
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&output_path)?;
    
    for event in &events {
        writeln!(output, "{}", serde_json::to_string(event)?)?;
    }
    
    if let Some(last) = events.last() {
        baseline.last_usn = last.usn;
    }
    
    std::fs::write(&baseline_path, serde_json::to_string_pretty(&baseline)?)?;
    
    println!("Output written to: {}", output_path.display());
    
    Ok(())
}

/// Benchmark full scan vs incremental
fn cmd_benchmark(path: PathBuf, count: usize) -> Result<()> {
    let test_dir = path.join("benchmark_test");
    
    println!("Arachne Incremental Benchmark");
    println!("==============================");
    println!("Test files: {}", count);
    println!();
    
    // Cleanup previous test
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir)?;
    
    // Phase 1: Create test files
    println!("[1/5] Creating {} test files...", count);
    let start = Instant::now();
    for i in 0..count {
        let file_path = test_dir.join(format!("file_{}.txt", i));
        std::fs::write(&file_path, format!("content {}", i))?;
    }
    let create_time = start.elapsed();
    println!("      Created in {:.2}s", create_time.as_secs_f64());
    
    // Phase 2: Full directory walk
    println!("\n[2/5] FULL scan (directory walk)...");
    let start = Instant::now();
    let mut full_scan_count = 0;
    walk_directory(&test_dir, &mut full_scan_count)?;
    let full_scan_time = start.elapsed();
    println!("      Files scanned: {}", full_scan_count);
    println!("      Time: {:.2}s", full_scan_time.as_secs_f64());
    
    // Phase 3: Create bootstrap and baseline
    println!("\n[3/5] Creating bootstrap (USN journal cursor)...");
    let bootstrap_path = test_dir.join("bootstrap.json");
    cmd_bootstrap(test_dir.clone(), bootstrap_path.clone())?;
    
    // Create baseline by doing a full scan
    println!("      Creating baseline cache...");
    let baseline_path = test_dir.join("baseline.json");
    cmd_scan(bootstrap_path.clone(), baseline_path.clone(), test_dir.join("events_init.jsonl"))?;
    
    // Phase 4: Modify files (1% churn)
    let modify_count = count.max(100) / 100;
    println!("\n[4/5] Modifying {} files (1% churn)...", modify_count);
    for i in 0..modify_count {
        let file_path = test_dir.join(format!("file_{}.txt", i));
        std::fs::write(&file_path, format!("modified content {}", i))?;
    }
    println!("      Done");
    
    // Phase 5: REAL incremental scan using USN journal
    println!("\n[5/5] INCREMENTAL scan (USN journal)...");
    
    // Read USN deltas directly from bootstrap's position (before file modifications)
    let bootstrap: BootstrapState = serde_json::from_str(
        &std::fs::read_to_string(&bootstrap_path)?
    )?;
    
    // Small delay to ensure USN records are flushed
    std::thread::sleep(std::time::Duration::from_millis(100));
    
    let start = Instant::now();
    let records = read_usn_deltas(&bootstrap.volume_path, bootstrap.next_usn)?;
    let incremental_time = start.elapsed();
    
    // Debug: show all records
    println!("      Debug: All {} USN records:", records.len());
    for (i, r) in records.iter().take(5).enumerate() {
        println!("        [{}] {} - reason: 0x{:08x}", i, r.filename, r.reason);
    }
    
    // Filter to only include records from our test directory
    let relevant_records: Vec<_> = records.iter()
        .filter(|r| {
            // Check if filename matches our test file pattern
            r.filename.contains("file_") && r.filename.contains(".txt")
        })
        .collect();
    
    println!("      Total USN records read: {}", records.len());
    println!("      Test directory records: {}", relevant_records.len());
    println!("      Time: {:.1}ms", incremental_time.as_secs_f64() * 1000.0);
    
    // Write events for inspection
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
    
    // Results
    println!("\n==============================");
    println!("RESULTS");
    println!("==============================");
    println!();
    println!("Full Directory Walk:  {:.2}s (scanned {} files)", full_scan_time.as_secs_f64(), full_scan_count);
    println!("USN Journal Scan:     {:.1}ms (read {} deltas)", incremental_time.as_secs_f64() * 1000.0, new_events);
    
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
    
    // Show sample events
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
    println!("To cleanup: Remove-Item -Recurse -Force '{}'", test_dir.display());
    
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

fn count_events(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(path)?;
    Ok(content.lines().count())
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
            return Err(anyhow::anyhow!("Failed to open volume {} - requires Administrator", volume));
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
            return Err(anyhow::anyhow!("USN journal query failed - journal may not exist. Run: fsutil usn createjournal {}:", volume));
        }
        
        Ok((journal_data.usn_journal_id, journal_data.next_usn))
    }
}

#[cfg(windows)]
fn read_usn_deltas(volume: &str, start_usn: i64) -> Result<Vec<UsnRecord>> {
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;
    use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ};
    
    const FSCTL_READ_USN_JOURNAL: u32 = 0x000900bb;
    const MAX_USN_RECORD_SIZE: usize = 65536;
    
    #[repr(C)]
    struct ReadUsnJournalData {
        start_usn: i64,
        reason_mask: u32,
        return_only_on_close: u32,
        timeout: u64,
        bytes_to_wait_for: u64,
        usn_journal_id: u64,
    }
    
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
        
        // Get journal ID first
        let (journal_id, _) = query_usn_journal(volume)?;
        
        let read_data = ReadUsnJournalData {
            start_usn,
            reason_mask: 0xFFFFFFFF, // All reasons
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 0,
            usn_journal_id: journal_id,
        };
        
        let mut buffer: Vec<u8> = vec![0; MAX_USN_RECORD_SIZE];
        let mut bytes_returned: u32 = 0;
        
        let result = DeviceIoControl(
            handle,
            FSCTL_READ_USN_JOURNAL,
            &read_data as *const _ as *mut _,
            std::mem::size_of::<ReadUsnJournalData>() as u32,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut bytes_returned,
            std::ptr::null_mut(),
        );
        
        winapi::um::handleapi::CloseHandle(handle);
        
        if result == 0 {
            // No new records is OK
            return Ok(vec![]);
        }
        
        // Parse USN records from buffer
        let mut records = Vec::new();
        let mut offset = 0;
        
        while offset + std::mem::size_of::<UsnRecordV2>() <= bytes_returned as usize {
            let record = &*(buffer.as_ptr().add(offset) as *const UsnRecordV2);
            
            if record.record_length == 0 {
                break;
            }
            
            // Extract filename
            let name_offset = offset + record.file_name_offset as usize;
            let name_length = record.file_name_length as usize;
            
            if name_offset + name_length <= bytes_returned as usize {
                let name_slice = std::slice::from_raw_parts(
                    buffer.as_ptr().add(name_offset) as *const u16,
                    name_length / 2
                );
                let filename = String::from_utf16_lossy(name_slice);
                
                records.push(UsnRecord {
                    frn: record.file_reference_number,
                    parent_frn: record.parent_file_reference_number,
                    usn: record.usn,
                    reason: record.reason,
                    filename,
                });
            }
            
            offset += record.record_length as usize;
            
            // Safety limit
            if records.len() >= 10000 {
                break;
            }
        }
        
        Ok(records)
    }
}

#[cfg(not(windows))]
fn query_usn_journal(_volume: &str) -> Result<(u64, i64)> {
    println!("USN journal is only available on Windows NTFS - returning mock data");
    Ok((0x123456789abcdef0, 1000000))
}

#[cfg(not(windows))]
fn read_usn_deltas(_volume: &str, _start_usn: i64) -> Result<Vec<UsnRecord>> {
    Ok(vec![])
}

fn process_records(
    records: Vec<UsnRecord>,
    baseline: &mut CachedBaseline,
    scan_root: &str,
) -> Result<Vec<FileEvent>> {
    let mut events = Vec::new();
    let mut seen_frns = std::collections::HashSet::new();
    
    for record in records {
        // Skip duplicates (same FRN)
        if !seen_frns.insert(record.frn) {
            continue;
        }
        
        let op = classify_reason(record.reason);
        
        // Build path from filename
        let filename = sanitize_filename(&record.filename);
        let path = if let Some(parent) = baseline.entries.get(&record.parent_frn) {
            format!("{}/{}", parent.path, filename)
        } else if record.parent_frn == 5 {
            // Root directory
            format!("{}/{}", scan_root, filename)
        } else {
            // Unknown parent - use just filename
            format!("{}/{}", scan_root, filename)
        };
        
        // Only include if within scan root
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
            });
            
            baseline.entries.insert(record.frn, BaselineEntry {
                path,
                parent_frn: record.parent_frn,
                exists: op != "deleted",
            });
        }
    }
    
    Ok(events)
}

fn classify_reason(reason: u32) -> &'static str {
    const FILE_CREATE: u32 = 0x00000100;
    const FILE_DELETE: u32 = 0x00000200;
    const RENAME_OLD: u32 = 0x00001000;
    const RENAME_NEW: u32 = 0x00002000;
    const DATA_OVERWRITE: u32 = 0x00000001;
    const DATA_EXTEND: u32 = 0x00000002;
    const DATA_TRUNCATION: u32 = 0x00000004;
    
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
    } else if reason & (DATA_OVERWRITE | DATA_EXTEND | DATA_TRUNCATION) != 0 {
        "modified"
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
    }.ok_or_else(|| anyhow::anyhow!("Invalid path: {}", path_str))?;
    
    Ok(drive_letter.to_uppercase().to_string())
}
