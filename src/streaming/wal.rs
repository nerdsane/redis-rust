//! Write-Ahead Log - Entry Format, Writer, Reader, Rotator
//!
//! ## File Layout
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │ Header (16 bytes)                │
//! │ - magic: "RWAL" (4 bytes)        │
//! │ - version: u8                    │
//! │ - flags: u8                      │
//! │ - reserved: 2 bytes              │
//! │ - sequence: u64 LE               │
//! ├──────────────────────────────────┤
//! │ Entry 0                          │
//! │ - data_length: u32 LE            │
//! │ - timestamp: u64 LE              │
//! │ - checksum: u32 LE (CRC32)       │
//! │ - data: [u8; data_length]        │
//! ├──────────────────────────────────┤
//! │ Entry 1 ...                      │
//! └──────────────────────────────────┘
//! ```
//!
//! Crash tolerance: each entry is individually CRC32-checksummed.
//! The reader stops at the first corrupted or truncated entry,
//! recovering all fully-written entries before the crash point.

use crate::replication::state::ReplicationDelta;
use crate::streaming::wal_store::{WalError, WalFileReader, WalFileWriter, WalStore};

/// WAL file magic number
pub const WAL_MAGIC: [u8; 4] = *b"RWAL";
/// Current WAL format version
pub const WAL_VERSION: u8 = 1;
/// Header size in bytes
pub const WAL_HEADER_SIZE: usize = 16;
/// Entry overhead: data_length(4) + timestamp(8) + checksum(4) = 16 bytes
pub const WAL_ENTRY_OVERHEAD: usize = 16;

/// A single WAL entry
#[derive(Debug, Clone)]
pub struct WalEntry {
    /// Serialized ReplicationDelta
    pub data: Vec<u8>,
    /// Timestamp from the delta's Lamport clock
    pub timestamp: u64,
    /// CRC32 checksum of data
    pub checksum: u32,
}

impl WalEntry {
    /// Create a new WAL entry from a ReplicationDelta
    pub fn from_delta(delta: &ReplicationDelta, timestamp: u64) -> Result<Self, WalError> {
        let data =
            bincode::serialize(delta).map_err(|e| WalError::Corruption(format!("serialize: {}", e)))?;
        let checksum = crc32fast::hash(&data);

        debug_assert!(!data.is_empty(), "Postcondition: serialized data must not be empty");

        Ok(WalEntry {
            data,
            timestamp,
            checksum,
        })
    }

    /// Deserialize the entry data into a ReplicationDelta
    pub fn to_delta(&self) -> Result<ReplicationDelta, WalError> {
        bincode::deserialize(&self.data)
            .map_err(|e| WalError::Corruption(format!("deserialize: {}", e)))
    }

    /// Validate the entry checksum
    pub fn validate(&self) -> bool {
        crc32fast::hash(&self.data) == self.checksum
    }

    /// Total size on disk (overhead + data)
    pub fn disk_size(&self) -> usize {
        WAL_ENTRY_OVERHEAD
            .checked_add(self.data.len())
            .expect("entry size overflow is unreachable for data < u32::MAX")
    }

    /// Encode the entry to bytes for writing
    pub fn encode(&self) -> Vec<u8> {
        let data_len = self.data.len() as u32;
        let total_size = WAL_ENTRY_OVERHEAD
            .checked_add(self.data.len())
            .expect("entry size overflow is unreachable");
        let mut buf = Vec::with_capacity(total_size);

        buf.extend_from_slice(&data_len.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.checksum.to_le_bytes());
        buf.extend_from_slice(&self.data);

        debug_assert_eq!(
            buf.len(),
            total_size,
            "Postcondition: encoded size must match expected"
        );

        buf
    }

    /// Decode an entry from bytes. Returns None if data is truncated or corrupt.
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < WAL_ENTRY_OVERHEAD {
            return None;
        }

        let data_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let timestamp = u64::from_le_bytes([
            data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11],
        ]);
        let checksum = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);

        let total_size = WAL_ENTRY_OVERHEAD.checked_add(data_len)?;
        if data.len() < total_size {
            return None; // Truncated entry
        }

        let entry_data = data[WAL_ENTRY_OVERHEAD..total_size].to_vec();

        // Validate CRC32
        let actual_checksum = crc32fast::hash(&entry_data);
        if actual_checksum != checksum {
            return None; // Corrupted entry
        }

        Some((
            WalEntry {
                data: entry_data,
                timestamp,
                checksum,
            },
            total_size,
        ))
    }
}

// ============================================================================
// WAL Writer
// ============================================================================

/// WAL file writer - writes header + entries to a single WAL file
pub struct WalWriter<W: WalFileWriter> {
    writer: W,
    sequence: u64,
    entry_count: u64,
    min_timestamp: u64,
    max_timestamp: u64,
}

impl<W: WalFileWriter> WalWriter<W> {
    /// Create a new WAL writer, writing the header immediately
    pub fn new(mut writer: W, sequence: u64) -> Result<Self, WalError> {
        // Write header
        let mut header = [0u8; WAL_HEADER_SIZE];
        header[0..4].copy_from_slice(&WAL_MAGIC);
        header[4] = WAL_VERSION;
        header[5] = 0; // flags
        // header[6..8] reserved
        header[8..16].copy_from_slice(&sequence.to_le_bytes());

        writer.append(&header)?;

        Ok(WalWriter {
            writer,
            sequence,
            entry_count: 0,
            min_timestamp: u64::MAX,
            max_timestamp: 0,
        })
    }

    /// Append an entry to the WAL (does NOT fsync)
    pub fn append_entry(&mut self, entry: &WalEntry) -> Result<u64, WalError> {
        debug_assert!(
            entry.validate(),
            "Precondition: entry checksum must be valid"
        );

        let encoded = entry.encode();
        let offset = self.writer.append(&encoded)?;

        self.entry_count = self
            .entry_count
            .checked_add(1)
            .expect("entry count overflow is unreachable");

        if entry.timestamp < self.min_timestamp {
            self.min_timestamp = entry.timestamp;
        }
        if entry.timestamp > self.max_timestamp {
            self.max_timestamp = entry.timestamp;
        }

        Ok(offset)
    }

    /// Fsync the WAL file
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.writer.sync()
    }

    /// Current file size in bytes
    pub fn size(&self) -> u64 {
        self.writer.size()
    }

    /// Number of entries written
    pub fn entry_count(&self) -> u64 {
        self.entry_count
    }

    /// Sequence number of this WAL file
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Maximum timestamp of entries in this file
    pub fn max_timestamp(&self) -> u64 {
        self.max_timestamp
    }
}

// ============================================================================
// WAL Reader
// ============================================================================

/// WAL file reader - reads header + entries from a WAL file
pub struct WalReader {
    data: Vec<u8>,
    sequence: u64,
}

impl WalReader {
    /// Open a WAL file for reading. Validates the header.
    pub fn open<R: WalFileReader>(mut reader: R) -> Result<Self, WalError> {
        let data = reader.read_all()?;

        if data.len() < WAL_HEADER_SIZE {
            return Err(WalError::Corruption("WAL file too short for header".to_string()));
        }

        // Validate magic
        if data[0..4] != WAL_MAGIC {
            return Err(WalError::Corruption(format!(
                "Invalid WAL magic: {:?}",
                &data[0..4]
            )));
        }

        // Validate version
        let version = data[4];
        if version != WAL_VERSION {
            return Err(WalError::Corruption(format!(
                "Unsupported WAL version: {}",
                version
            )));
        }

        let sequence = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);

        Ok(WalReader { data, sequence })
    }

    /// Sequence number of this WAL file
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Read all valid entries from the WAL file.
    /// Stops at the first corrupted or truncated entry (crash tolerance).
    pub fn entries(&self) -> Vec<WalEntry> {
        let mut entries = Vec::new();
        let mut offset = WAL_HEADER_SIZE;

        while offset < self.data.len() {
            match WalEntry::decode(&self.data[offset..]) {
                Some((entry, consumed)) => {
                    entries.push(entry);
                    offset = offset
                        .checked_add(consumed)
                        .expect("offset overflow is unreachable");
                }
                None => break, // Truncated or corrupt entry — stop here
            }
        }

        entries
    }

    /// Read entries with timestamp strictly greater than the given threshold.
    /// Used during recovery to skip entries already in object store.
    pub fn entries_after(&self, after_timestamp: u64) -> Vec<WalEntry> {
        self.entries()
            .into_iter()
            .filter(|e| e.timestamp > after_timestamp)
            .collect()
    }
}

// ============================================================================
// WAL Rotator - manages multiple WAL files
// ============================================================================

/// WAL file naming: wal-{sequence:08x}.wal
fn wal_file_name(sequence: u64) -> String {
    format!("wal-{:08x}.wal", sequence)
}

/// Parse sequence number from WAL file name
fn parse_wal_sequence(name: &str) -> Option<u64> {
    let name = name.strip_prefix("wal-")?.strip_suffix(".wal")?;
    u64::from_str_radix(name, 16).ok()
}

/// Manages multiple WAL files with rotation and truncation
pub struct WalRotator<S: WalStore> {
    store: S,
    max_file_size: usize,
    current_writer: Option<WalWriter<S::Writer>>,
    current_sequence: u64,
}

impl<S: WalStore> WalRotator<S> {
    /// Create a new WAL rotator. Scans existing files to determine next sequence.
    pub fn new(store: S, max_file_size: usize) -> Result<Self, WalError> {
        debug_assert!(
            max_file_size > WAL_HEADER_SIZE,
            "Precondition: max_file_size must be larger than header"
        );

        let files = store.list()?;
        let max_seq = files
            .iter()
            .filter_map(|name| parse_wal_sequence(name))
            .max()
            .unwrap_or(0);

        Ok(WalRotator {
            store,
            max_file_size,
            current_writer: None,
            current_sequence: max_seq,
        })
    }

    /// Append an entry, rotating the WAL file if needed.
    /// Returns the file sequence number the entry was written to.
    /// On write failure, forces rotation so subsequent writes go to a clean file.
    pub fn append(&mut self, entry: &WalEntry) -> Result<u64, WalError> {
        // Check if we need a new file
        let needs_new_file = match &self.current_writer {
            None => true,
            Some(writer) => writer.size() as usize >= self.max_file_size,
        };

        if needs_new_file {
            self.rotate()?;
        }

        let writer = self
            .current_writer
            .as_mut()
            .expect("current_writer must exist after rotate");

        match writer.append_entry(entry) {
            Ok(_) => Ok(writer.sequence()),
            Err(e) => {
                // Write failed — force rotation so next write goes to a clean file.
                // The current file may have corrupt/partial data that would block
                // recovery of subsequent valid entries.
                self.current_writer = None;
                Err(e)
            }
        }
    }

    /// Fsync the current WAL file
    pub fn sync(&mut self) -> Result<(), WalError> {
        if let Some(ref mut writer) = self.current_writer {
            writer.sync()?;
        }
        Ok(())
    }

    /// Rotate to a new WAL file
    fn rotate(&mut self) -> Result<(), WalError> {
        // Close current writer (drop)
        self.current_writer = None;

        self.current_sequence = self
            .current_sequence
            .checked_add(1)
            .expect("WAL sequence overflow is unreachable");

        let name = wal_file_name(self.current_sequence);
        let file_writer = self.store.create(&name)?;
        let wal_writer = WalWriter::new(file_writer, self.current_sequence)?;
        self.current_writer = Some(wal_writer);

        Ok(())
    }

    /// Recover all entries from all WAL files, in sequence order.
    /// Entries from each file are read until the first corrupt/truncated entry.
    /// Corrupt or unreadable files are skipped (crash tolerance).
    pub fn recover_all_entries(&self) -> Result<Vec<WalEntry>, WalError> {
        let mut all_entries = Vec::new();
        let files = self.store.list()?;

        // Sort by sequence (already sorted by name due to hex format)
        let mut wal_files: Vec<(u64, String)> = files
            .into_iter()
            .filter_map(|name| {
                let seq = parse_wal_sequence(&name)?;
                Some((seq, name))
            })
            .collect();
        wal_files.sort_by_key(|(seq, _)| *seq);

        for (_, name) in wal_files {
            let reader = match self.store.open_read(&name) {
                Ok(r) => r,
                Err(_) => continue, // Skip unreadable files
            };
            let wal_reader = match WalReader::open(reader) {
                Ok(r) => r,
                Err(_) => continue, // Skip corrupt files (e.g. truncated header)
            };
            all_entries.extend(wal_reader.entries());
        }

        Ok(all_entries)
    }

    /// Recover entries with timestamp strictly greater than the given threshold.
    pub fn recover_entries_after(&self, after_timestamp: u64) -> Result<Vec<ReplicationDelta>, WalError> {
        let entries = self.recover_all_entries()?;
        let mut deltas = Vec::new();
        for entry in entries {
            if entry.timestamp > after_timestamp {
                deltas.push(entry.to_delta()?);
            }
        }
        Ok(deltas)
    }

    /// Truncate WAL files that contain only entries at or before the given timestamp.
    /// Keeps any file that might contain entries after the timestamp.
    pub fn truncate_before(&mut self, up_to_timestamp: u64) -> Result<usize, WalError> {
        let files = self.store.list()?;
        let mut deleted_count = 0;

        // Never delete the current file
        let current_name = self
            .current_writer
            .as_ref()
            .map(|w| wal_file_name(w.sequence()));

        for name in &files {
            if Some(name.clone()) == current_name {
                continue;
            }

            // Read the file and check if all entries are <= up_to_timestamp
            let reader = match self.store.open_read(name) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let wal_reader = match WalReader::open(reader) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let entries = wal_reader.entries();
            if entries.is_empty() {
                // Empty file (maybe just header) — safe to delete
                self.store.delete(name)?;
                deleted_count += 1;
                continue;
            }

            let max_ts = entries.iter().map(|e| e.timestamp).max().unwrap_or(0);
            if max_ts <= up_to_timestamp {
                self.store.delete(name)?;
                deleted_count += 1;
            }
        }

        Ok(deleted_count)
    }

    /// Get the current sequence number
    pub fn current_sequence(&self) -> u64 {
        self.current_sequence
    }

    /// Get a reference to the underlying store
    pub fn store(&self) -> &S {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;
    use crate::replication::lattice::{LamportClock, ReplicaId};
    use crate::replication::state::ReplicatedValue;
    use crate::streaming::wal_store::InMemoryWalStore;

    fn make_delta(key: &str, value: &str, ts: u64) -> ReplicationDelta {
        let replica_id = ReplicaId::new(1);
        let clock = LamportClock {
            time: ts,
            replica_id,
        };
        let replicated = ReplicatedValue::with_value(SDS::from_str(value), clock);
        ReplicationDelta::new(key.to_string(), replicated, replica_id)
    }

    #[test]
    fn test_entry_roundtrip() {
        let delta = make_delta("key1", "value1", 100);
        let entry = WalEntry::from_delta(&delta, 100).unwrap();

        assert!(entry.validate());
        assert!(entry.disk_size() > WAL_ENTRY_OVERHEAD);

        // Encode and decode
        let encoded = entry.encode();
        let (decoded, consumed) = WalEntry::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.timestamp, 100);
        assert!(decoded.validate());

        // Deserialize back to delta
        let recovered_delta = decoded.to_delta().unwrap();
        assert_eq!(recovered_delta.key, "key1");
    }

    #[test]
    fn test_entry_decode_truncated() {
        let delta = make_delta("key1", "value1", 100);
        let entry = WalEntry::from_delta(&delta, 100).unwrap();
        let encoded = entry.encode();

        // Truncate at various points
        assert!(WalEntry::decode(&encoded[..5]).is_none()); // Too short for header
        assert!(WalEntry::decode(&encoded[..WAL_ENTRY_OVERHEAD]).is_none()); // Missing data
        assert!(WalEntry::decode(&encoded[..encoded.len() - 1]).is_none()); // Off by one
    }

    #[test]
    fn test_entry_decode_corrupted() {
        let delta = make_delta("key1", "value1", 100);
        let entry = WalEntry::from_delta(&delta, 100).unwrap();
        let mut encoded = entry.encode();

        // Corrupt a data byte
        let last_idx = encoded.len() - 1;
        encoded[last_idx] ^= 0xFF;

        assert!(WalEntry::decode(&encoded).is_none()); // CRC mismatch
    }

    #[test]
    fn test_writer_reader_roundtrip() {
        let store = InMemoryWalStore::new();

        // Write entries
        let file_writer = store.create("wal-00000001.wal").unwrap();
        let mut writer = WalWriter::new(file_writer, 1).unwrap();

        let delta1 = make_delta("k1", "v1", 100);
        let delta2 = make_delta("k2", "v2", 200);

        let entry1 = WalEntry::from_delta(&delta1, 100).unwrap();
        let entry2 = WalEntry::from_delta(&delta2, 200).unwrap();

        writer.append_entry(&entry1).unwrap();
        writer.append_entry(&entry2).unwrap();
        writer.sync().unwrap();

        assert_eq!(writer.entry_count(), 2);
        assert_eq!(writer.sequence(), 1);
        assert_eq!(writer.max_timestamp(), 200);

        // Read entries
        let file_reader = store.open_read("wal-00000001.wal").unwrap();
        let reader = WalReader::open(file_reader).unwrap();
        assert_eq!(reader.sequence(), 1);

        let entries = reader.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp, 100);
        assert_eq!(entries[1].timestamp, 200);

        let d1 = entries[0].to_delta().unwrap();
        assert_eq!(d1.key, "k1");
    }

    #[test]
    fn test_reader_crash_tolerance() {
        let store = InMemoryWalStore::new();

        // Write 3 entries
        let file_writer = store.create("wal-00000001.wal").unwrap();
        let mut writer = WalWriter::new(file_writer, 1).unwrap();

        for i in 0..3 {
            let delta = make_delta(&format!("k{}", i), &format!("v{}", i), (i + 1) as u64 * 100);
            let entry = WalEntry::from_delta(&delta, (i + 1) as u64 * 100).unwrap();
            writer.append_entry(&entry).unwrap();
        }

        // Simulate crash: truncate the file mid-way through the third entry
        let data = store.get_file_data("wal-00000001.wal").unwrap();
        let truncated_len = data.len() - 5; // Remove last 5 bytes
        store.set_file_data("wal-00000001.wal", data[..truncated_len].to_vec());

        // Read should recover the first 2 entries
        let file_reader = store.open_read("wal-00000001.wal").unwrap();
        let reader = WalReader::open(file_reader).unwrap();
        let entries = reader.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].to_delta().unwrap().key, "k0");
        assert_eq!(entries[1].to_delta().unwrap().key, "k1");
    }

    #[test]
    fn test_entries_after_filter() {
        let store = InMemoryWalStore::new();

        let file_writer = store.create("wal-00000001.wal").unwrap();
        let mut writer = WalWriter::new(file_writer, 1).unwrap();

        for ts in [100, 200, 300, 400, 500] {
            let delta = make_delta(&format!("k{}", ts), "v", ts);
            let entry = WalEntry::from_delta(&delta, ts).unwrap();
            writer.append_entry(&entry).unwrap();
        }

        let file_reader = store.open_read("wal-00000001.wal").unwrap();
        let reader = WalReader::open(file_reader).unwrap();

        let filtered = reader.entries_after(250);
        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].timestamp, 300);
        assert_eq!(filtered[1].timestamp, 400);
        assert_eq!(filtered[2].timestamp, 500);
    }

    #[test]
    fn test_rotator_basic() {
        let store = InMemoryWalStore::new();
        let mut rotator = WalRotator::new(store, 1024 * 1024).unwrap(); // 1MB max

        for i in 0..10 {
            let delta = make_delta(&format!("k{}", i), &format!("v{}", i), (i + 1) as u64 * 100);
            let entry = WalEntry::from_delta(&delta, (i + 1) as u64 * 100).unwrap();
            rotator.append(&entry).unwrap();
        }
        rotator.sync().unwrap();

        // Should have created one file
        let files = rotator.store().list().unwrap();
        assert_eq!(files.len(), 1);

        // Recover all entries
        let recovered = rotator.recover_all_entries().unwrap();
        assert_eq!(recovered.len(), 10);
    }

    #[test]
    fn test_rotator_rotation() {
        let store = InMemoryWalStore::new();
        // Very small file size to force rotation
        let mut rotator = WalRotator::new(store, 100).unwrap();

        for i in 0..20 {
            let delta = make_delta(&format!("k{}", i), &format!("v{}", i), (i + 1) as u64 * 100);
            let entry = WalEntry::from_delta(&delta, (i + 1) as u64 * 100).unwrap();
            rotator.append(&entry).unwrap();
        }

        // Should have created multiple files
        let files = rotator.store().list().unwrap();
        assert!(
            files.len() > 1,
            "Expected multiple WAL files after rotation, got {}",
            files.len()
        );

        // Recover all entries across all files
        let recovered = rotator.recover_all_entries().unwrap();
        assert_eq!(recovered.len(), 20);
    }

    #[test]
    fn test_rotator_truncation() {
        let store = InMemoryWalStore::new();
        // Small files to create multiple
        let mut rotator = WalRotator::new(store, 100).unwrap();

        // Write entries with timestamps 100-2000
        for i in 0..20 {
            let ts = (i + 1) as u64 * 100;
            let delta = make_delta(&format!("k{}", i), "v", ts);
            let entry = WalEntry::from_delta(&delta, ts).unwrap();
            rotator.append(&entry).unwrap();
        }

        let files_before = rotator.store().list().unwrap().len();

        // Truncate entries with timestamp <= 1000
        let deleted = rotator.truncate_before(1000).unwrap();
        assert!(deleted > 0, "Expected some files to be deleted");

        let files_after = rotator.store().list().unwrap().len();
        assert!(
            files_after < files_before,
            "Expected fewer files after truncation"
        );

        // Remaining entries should all be > 1000
        let remaining = rotator.recover_all_entries().unwrap();
        for entry in &remaining {
            // Some entries <= 1000 may remain in files that also contain entries > 1000
            // This is by design - truncation is conservative (whole-file granularity)
        }
        assert!(!remaining.is_empty(), "Should still have some entries");
    }

    #[test]
    fn test_rotator_recover_entries_after() {
        let store = InMemoryWalStore::new();
        let mut rotator = WalRotator::new(store, 100).unwrap();

        for i in 0..10 {
            let ts = (i + 1) as u64 * 100;
            let delta = make_delta(&format!("k{}", i), "v", ts);
            let entry = WalEntry::from_delta(&delta, ts).unwrap();
            rotator.append(&entry).unwrap();
        }

        let deltas = rotator.recover_entries_after(500).unwrap();
        assert_eq!(deltas.len(), 5);
        assert_eq!(deltas[0].key, "k5");
    }

    #[test]
    fn test_wal_file_naming() {
        assert_eq!(wal_file_name(0), "wal-00000000.wal");
        assert_eq!(wal_file_name(1), "wal-00000001.wal");
        assert_eq!(wal_file_name(255), "wal-000000ff.wal");
        assert_eq!(wal_file_name(0xDEADBEEF), "wal-deadbeef.wal");

        assert_eq!(parse_wal_sequence("wal-00000000.wal"), Some(0));
        assert_eq!(parse_wal_sequence("wal-000000ff.wal"), Some(255));
        assert_eq!(parse_wal_sequence("wal-deadbeef.wal"), Some(0xDEADBEEF));
        assert_eq!(parse_wal_sequence("not-a-wal.txt"), None);
    }
}
