# Arachne Incremental

Fast incremental file change detection for Windows NTFS using the USN Change Journal.

**Problem:** Full filesystem scans take 30-120 minutes on large directories.
**Solution:** USN journal-based deltas in under 1 second.

## Quick Start

```powershell
# 1. Install (requires Administrator)
irm https://github.com/Honkware/arachne-incremental/releases/latest/download/arachne-incremental.exe -OutFile arachne-incremental.exe

# 2. Create bootstrap state
.\arachne-incremental.exe bootstrap --path C:\data --output bootstrap.json

# 3. Run incremental scan
.\arachne-incremental.exe scan --bootstrap bootstrap.json --baseline baseline.json --output changes.jsonl
```

## How It Works

```
Full Scan (Directory Walk)     vs     Incremental Scan (USN Journal)
=========================              =============================
Walk every file: 30-120 min            Read journal deltas: <1 sec
CPU: 100%                              CPU: <5%
Disk: Heavy I/O                        Disk: Minimal
                                       
Speedup: 99%+ latency reduction
```

## Usage

### 1. Bootstrap (One-time per volume)

```powershell
arachne-incremental bootstrap --path C:\data --output bootstrap.json
```

Creates a cursor pointing to the current USN journal position.

### 2. Incremental Scan

```powershell
arachne-incremental scan --bootstrap bootstrap.json --baseline baseline.json --output changes.jsonl
```

Reads only the changes since last scan.

### 3. Process Events

```python
import json

with open('changes.jsonl') as f:
    for line in f:
        event = json.loads(line)
        print(f"{event['op']}: {event['path']}")
```

## Output Format

**changes.jsonl** (JSON Lines):

```json
{"op":"modified","path":"C:\\data\\file.txt","frn":12345,"usn":9876543210}
{"op":"renamed","path":"C:\\data\\new.txt","old_path":"C:\\data\\old.txt","frn":12346,"usn":9876543211}
{"op":"deleted","path":"C:\\data\\gone.txt","frn":12347,"usn":9876543212}
```

| Field | Description |
|-------|-------------|
| `op` | `created`, `modified`, `renamed`, `deleted` |
| `path` | Current file path |
| `old_path` | Previous path (renames only) |
| `frn` | NTFS File Reference Number (unique ID) |
| `usn` | USN sequence number |

## Benchmark

```powershell
arachne-incremental benchmark --path C:\temp --count 10000
```

**Results on Azure VM (8 vCPU, 32GB RAM):**

| Files | Full Scan | Incremental | Reduction |
|-------|-----------|-------------|-----------|
| 1,000 | 12.45s | 720ms | **94.2%** |
| 10,000 | 45.20s | 1.2s | **97.3%** |
| 100,000 | 8.5min | 2.8s | **99.4%** |
| 1,000,000 | 95min | 8.5s | **99.8%** |

## Requirements

- Windows 10/11 or Server 2016+
- NTFS filesystem
- Administrator privileges (for USN journal access)
- USN journal enabled (auto-created if missing)

## Building from Source

```bash
git clone https://github.com/Honkware/arachne-incremental.git
cd arachne-incremental
cargo build --release
```

## Use Cases

### Backup Acceleration
```powershell
# Pre-backup: get changed files only
arachne-incremental scan --bootstrap bootstrap.json --baseline baseline.json --output changes.jsonl

# Feed to backup tool
Get-Content changes.jsonl | ConvertFrom-Json | ForEach-Object { Backup-File $_.path }
```

### File Sync
```powershell
# Continuous sync
while ($true) {
    arachne-incremental scan --bootstrap bootstrap.json --baseline baseline.json --output changes.jsonl
    Sync-Changes changes.jsonl
    Start-Sleep -Seconds 5
}
```

### Monitoring
```powershell
# Detect changes in real-time
arachne-incremental scan --bootstrap bootstrap.json --baseline baseline.json --output changes.jsonl
Get-Content changes.jsonl | ForEach-Object { Send-Alert $_ }
```

## How It Works (Technical)

NTFS maintains a change journal (USN) that records every file operation:

```
USN Journal
===========
USN 100: File A created
USN 101: File B modified
USN 102: File C renamed
USN 103: File D deleted
...
```

Arachne Incremental:
1. **Bootstrap**: Records current USN position
2. **Scan**: Reads only new entries (USN 101 → current)
3. **Process**: Converts raw records to structured events
4. **Cache**: Maintains FRN → path mapping for renames

## License

MIT
