//! Segment File Format
//!
//! Segments are immutable files containing batched ReplicationDeltas.
//! Format uses bincode for efficient serialization and CRC32 for integrity.
//!
//! ## File Layout
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │ Header (fixed size)              │
//! │ - magic: "RSEG"                  │
//! │ - version: u8                    │
//! │ - flags: u8                      │
//! │ - record_count: u32              │
//! │ - timestamps: u64 x 2            │
//! │ - header_checksum: u32           │
//! ├──────────────────────────────────┤
//! │ Records (variable)               │
//! │ - length: u32                    │
//! │ - data: bincode(Delta)           │
//! ├──────────────────────────────────┤
//! │ Footer (fixed size)              │
//! │ - data_checksum: u32             │
//! │ - sizes: u64 x 2                 │
//! │ - footer_magic: "GESR"           │
//! └──────────────────────────────────┘
//! ```

use crate::replication::state::ReplicationDelta;
use serde::{Deserialize, Serialize};

/// Segment file magic number
pub const SEGMENT_MAGIC: [u8; 4] = *b"RSEG";
/// Reversed magic for footer validation
pub const FOOTER_MAGIC: [u8; 4] = *b"GESR";
/// Current segment format version
pub const SEGMENT_VERSION: u8 = 1;

/// Header size in bytes
const HEADER_SIZE: usize = 40;
/// Footer size in bytes
const FOOTER_SIZE: usize = 24;

/// Compression options for segments
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// No compression
    #[default]
    None,
    /// Zstd compression (requires feature)
    #[cfg(feature = "compression")]
    Zstd { level: i32 },
}

impl Compression {
    /// Flag value for header
    fn flag(&self) -> u8 {
        match self {
            Compression::None => 0,
            #[cfg(feature = "compression")]
            Compression::Zstd { .. } => 1,
        }
    }

    /// Parse from flag value
    fn from_flag(flag: u8) -> Option<Self> {
        match flag {
            0 => Some(Compression::None),
            #[cfg(feature = "compression")]
            1 => Some(Compression::Zstd { level: 3 }),
            _ => None,
        }
    }
}

/// Segment error types
#[derive(Debug)]
pub enum SegmentError {
    /// Invalid magic number
    InvalidMagic,
    /// Unsupported version
    UnsupportedVersion(u8),
    /// Checksum mismatch
    ChecksumMismatch { expected: u32, actual: u32 },
    /// Serialization error
    Serialization(String),
    /// I/O error
    Io(std::io::Error),
    /// Segment is empty
    Empty,
    /// Unsupported compression
    UnsupportedCompression(u8),
}

impl std::fmt::Display for SegmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SegmentError::InvalidMagic => write!(f, "Invalid segment magic number"),
            SegmentError::UnsupportedVersion(v) => write!(f, "Unsupported segment version: {}", v),
            SegmentError::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "Checksum mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            SegmentError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            SegmentError::Io(e) => write!(f, "I/O error: {}", e),
            SegmentError::Empty => write!(f, "Segment is empty"),
            SegmentError::UnsupportedCompression(c) => {
                write!(f, "Unsupported compression flag: {}", c)
            }
        }
    }
}

impl std::error::Error for SegmentError {}

impl From<std::io::Error> for SegmentError {
    fn from(e: std::io::Error) -> Self {
        SegmentError::Io(e)
    }
}

impl From<bincode::Error> for SegmentError {
    fn from(e: bincode::Error) -> Self {
        SegmentError::Serialization(e.to_string())
    }
}

/// Segment header (serialized at start of file)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentHeader {
    /// Magic number for format identification
    pub magic: [u8; 4],
    /// Format version
    pub version: u8,
    /// Flags (bit 0: compressed)
    pub flags: u8,
    /// Number of records in segment
    pub record_count: u32,
    /// Earliest delta timestamp (Lamport time)
    pub min_timestamp: u64,
    /// Latest delta timestamp (Lamport time)
    pub max_timestamp: u64,
    /// CRC32 of header fields (excluding this field)
    pub header_checksum: u32,
}

impl SegmentHeader {
    /// Create a new header
    fn new(record_count: u32, min_ts: u64, max_ts: u64, compression: Compression) -> Self {
        let mut header = SegmentHeader {
            magic: SEGMENT_MAGIC,
            version: SEGMENT_VERSION,
            flags: compression.flag(),
            record_count,
            min_timestamp: min_ts,
            max_timestamp: max_ts,
            header_checksum: 0,
        };
        header.header_checksum = header.compute_checksum();
        header
    }

    /// Compute checksum of header fields
    fn compute_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&self.magic);
        hasher.update(&[self.version, self.flags]);
        hasher.update(&self.record_count.to_le_bytes());
        hasher.update(&self.min_timestamp.to_le_bytes());
        hasher.update(&self.max_timestamp.to_le_bytes());
        hasher.finalize()
    }

    /// Validate header
    fn validate(&self) -> Result<(), SegmentError> {
        if self.magic != SEGMENT_MAGIC {
            return Err(SegmentError::InvalidMagic);
        }
        if self.version != SEGMENT_VERSION {
            return Err(SegmentError::UnsupportedVersion(self.version));
        }
        let expected = self.compute_checksum();
        if self.header_checksum != expected {
            return Err(SegmentError::ChecksumMismatch {
                expected,
                actual: self.header_checksum,
            });
        }
        Ok(())
    }

    /// Serialize header to bytes (fixed size)
    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE);
        buf.extend_from_slice(&self.magic);
        buf.push(self.version);
        buf.push(self.flags);
        buf.extend_from_slice(&self.record_count.to_le_bytes());
        buf.extend_from_slice(&self.min_timestamp.to_le_bytes());
        buf.extend_from_slice(&self.max_timestamp.to_le_bytes());
        buf.extend_from_slice(&self.header_checksum.to_le_bytes());
        // Pad to fixed size
        buf.resize(HEADER_SIZE, 0);
        buf
    }

    /// Parse header from bytes
    ///
    /// # Safety Invariant
    /// After the length check, all slice operations are guaranteed to succeed
    /// because HEADER_SIZE (64) > all slice indices used below.
    fn from_bytes(data: &[u8]) -> Result<Self, SegmentError> {
        if data.len() < HEADER_SIZE {
            return Err(SegmentError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Header too short",
            )));
        }

        // TigerStyle: All try_into() calls are safe after length validation above.
        // HEADER_SIZE=64 ensures indices 0..30 are valid.
        let magic: [u8; 4] = data[0..4]
            .try_into()
            .expect("length validated: HEADER_SIZE >= 4");
        let version = data[4];
        let flags = data[5];
        let record_count = u32::from_le_bytes(
            data[6..10]
                .try_into()
                .expect("length validated: HEADER_SIZE >= 10"),
        );
        let min_timestamp = u64::from_le_bytes(
            data[10..18]
                .try_into()
                .expect("length validated: HEADER_SIZE >= 18"),
        );
        let max_timestamp = u64::from_le_bytes(
            data[18..26]
                .try_into()
                .expect("length validated: HEADER_SIZE >= 26"),
        );
        let header_checksum = u32::from_le_bytes(
            data[26..30]
                .try_into()
                .expect("length validated: HEADER_SIZE >= 30"),
        );

        Ok(SegmentHeader {
            magic,
            version,
            flags,
            record_count,
            min_timestamp,
            max_timestamp,
            header_checksum,
        })
    }
}

/// Segment footer (serialized at end of file)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentFooter {
    /// CRC32 of all record data
    pub data_checksum: u32,
    /// Uncompressed size of records
    pub uncompressed_size: u64,
    /// Compressed size (same as uncompressed if no compression)
    pub compressed_size: u64,
    /// Reversed magic for validation
    pub footer_magic: [u8; 4],
}

impl SegmentFooter {
    /// Create a new footer
    fn new(data_checksum: u32, uncompressed_size: u64, compressed_size: u64) -> Self {
        SegmentFooter {
            data_checksum,
            uncompressed_size,
            compressed_size,
            footer_magic: FOOTER_MAGIC,
        }
    }

    /// Serialize footer to bytes (fixed size)
    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(FOOTER_SIZE);
        buf.extend_from_slice(&self.data_checksum.to_le_bytes());
        buf.extend_from_slice(&self.uncompressed_size.to_le_bytes());
        buf.extend_from_slice(&self.compressed_size.to_le_bytes());
        buf.extend_from_slice(&self.footer_magic);
        buf
    }

    /// Parse footer from bytes
    ///
    /// # Safety Invariant
    /// After the length check, all slice operations are guaranteed to succeed
    /// because FOOTER_SIZE (24) >= all slice indices used below.
    fn from_bytes(data: &[u8]) -> Result<Self, SegmentError> {
        if data.len() < FOOTER_SIZE {
            return Err(SegmentError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Footer too short",
            )));
        }

        // TigerStyle: All try_into() calls are safe after length validation above.
        // FOOTER_SIZE=24 ensures indices 0..24 are valid.
        let data_checksum = u32::from_le_bytes(
            data[0..4]
                .try_into()
                .expect("length validated: FOOTER_SIZE >= 4"),
        );
        let uncompressed_size = u64::from_le_bytes(
            data[4..12]
                .try_into()
                .expect("length validated: FOOTER_SIZE >= 12"),
        );
        let compressed_size = u64::from_le_bytes(
            data[12..20]
                .try_into()
                .expect("length validated: FOOTER_SIZE >= 20"),
        );
        let footer_magic: [u8; 4] = data[20..24]
            .try_into()
            .expect("length validated: FOOTER_SIZE >= 24");

        if footer_magic != FOOTER_MAGIC {
            return Err(SegmentError::InvalidMagic);
        }

        Ok(SegmentFooter {
            data_checksum,
            uncompressed_size,
            compressed_size,
            footer_magic,
        })
    }
}

/// A complete segment (header + records + footer)
#[derive(Debug, Clone)]
pub struct Segment {
    /// Segment header
    pub header: SegmentHeader,
    /// Segment footer
    pub footer: SegmentFooter,
    /// Raw record data (between header and footer)
    record_data: Vec<u8>,
}

impl Segment {
    /// Get the number of records
    pub fn record_count(&self) -> u32 {
        self.header.record_count
    }

    /// Get minimum timestamp
    pub fn min_timestamp(&self) -> u64 {
        self.header.min_timestamp
    }

    /// Get maximum timestamp
    pub fn max_timestamp(&self) -> u64 {
        self.header.max_timestamp
    }

    /// Get total size in bytes
    pub fn size_bytes(&self) -> usize {
        HEADER_SIZE + self.record_data.len() + FOOTER_SIZE
    }
}

/// Writer for creating segments
pub struct SegmentWriter {
    compression: Compression,
    records: Vec<Vec<u8>>,
    min_timestamp: u64,
    max_timestamp: u64,
    total_size: usize,
}

impl SegmentWriter {
    /// Create a new segment writer
    pub fn new(compression: Compression) -> Self {
        SegmentWriter {
            compression,
            records: Vec::new(),
            min_timestamp: u64::MAX,
            max_timestamp: 0,
            total_size: 0,
        }
    }

    /// TigerStyle: Verify all invariants hold
    ///
    /// # Invariants
    /// - If non-empty: min_timestamp <= max_timestamp
    /// - total_size == sum of all record lengths
    #[cfg(debug_assertions)]
    fn verify_invariants(&self) {
        // Non-empty segment must have valid timestamp range
        if !self.records.is_empty() {
            debug_assert!(
                self.min_timestamp <= self.max_timestamp,
                "Invariant violated: min_timestamp ({}) must be <= max_timestamp ({})",
                self.min_timestamp,
                self.max_timestamp
            );
        }

        // total_size must match sum of records
        let actual_size: usize = self.records.iter().map(|r| r.len()).sum();
        debug_assert_eq!(
            self.total_size, actual_size,
            "Invariant violated: total_size ({}) must equal sum of record lengths ({})",
            self.total_size, actual_size
        );
    }

    /// Add a delta to the segment
    pub fn write_delta(&mut self, delta: &ReplicationDelta) -> Result<(), SegmentError> {
        // Serialize delta with bincode
        let data = bincode::serialize(delta)?;

        // Update timestamps
        let ts = delta.value.timestamp.time;
        self.min_timestamp = self.min_timestamp.min(ts);
        self.max_timestamp = self.max_timestamp.max(ts);

        // Store record (length-prefixed)
        let len = data.len() as u32;
        let mut record = Vec::with_capacity(4 + data.len());
        record.extend_from_slice(&len.to_le_bytes());
        record.extend_from_slice(&data);

        self.total_size += record.len();
        self.records.push(record);

        #[cfg(debug_assertions)]
        self.verify_invariants();

        Ok(())
    }

    /// Get estimated size of the segment
    pub fn estimated_size(&self) -> usize {
        HEADER_SIZE + self.total_size + FOOTER_SIZE
    }

    /// Get number of records written so far
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Check if the writer is empty
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Finalize and return the complete segment bytes
    pub fn finish(self) -> Result<Vec<u8>, SegmentError> {
        if self.records.is_empty() {
            return Err(SegmentError::Empty);
        }

        // Concatenate all records
        let mut record_data = Vec::with_capacity(self.total_size);
        for record in &self.records {
            record_data.extend_from_slice(record);
        }

        // Compute data checksum
        let data_checksum = crc32fast::hash(&record_data);
        let uncompressed_size = record_data.len() as u64;

        // Apply compression if enabled
        let (final_data, compressed_size) = match self.compression {
            Compression::None => {
                let size = record_data.len() as u64;
                (record_data, size)
            }
            #[cfg(feature = "compression")]
            Compression::Zstd { level } => {
                let compressed = zstd::encode_all(record_data.as_slice(), level)?;
                let size = compressed.len() as u64;
                (compressed, size)
            }
        };

        // Create header and footer
        let header = SegmentHeader::new(
            self.records.len() as u32,
            self.min_timestamp,
            self.max_timestamp,
            self.compression,
        );
        let footer = SegmentFooter::new(data_checksum, uncompressed_size, compressed_size);

        // Assemble final segment
        let mut segment = Vec::with_capacity(HEADER_SIZE + final_data.len() + FOOTER_SIZE);
        segment.extend_from_slice(&header.to_bytes());
        segment.extend_from_slice(&final_data);
        segment.extend_from_slice(&footer.to_bytes());

        Ok(segment)
    }
}

/// Reader for parsing segments
pub struct SegmentReader {
    segment: Segment,
    compression: Compression,
}

impl SegmentReader {
    /// Open a segment from bytes
    pub fn open(data: &[u8]) -> Result<Self, SegmentError> {
        if data.len() < HEADER_SIZE + FOOTER_SIZE {
            return Err(SegmentError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Segment too small",
            )));
        }

        // Parse header
        let header = SegmentHeader::from_bytes(&data[..HEADER_SIZE])?;
        header.validate()?;

        // Parse footer
        let footer_start = data.len() - FOOTER_SIZE;
        let footer = SegmentFooter::from_bytes(&data[footer_start..])?;

        // Get compression type
        let compression = Compression::from_flag(header.flags)
            .ok_or(SegmentError::UnsupportedCompression(header.flags))?;

        // Extract record data
        let record_data = data[HEADER_SIZE..footer_start].to_vec();

        Ok(SegmentReader {
            segment: Segment {
                header,
                footer,
                record_data,
            },
            compression,
        })
    }

    /// Validate segment checksums
    pub fn validate(&self) -> Result<(), SegmentError> {
        // Decompress if needed
        let decompressed = self.decompress_data()?;

        // Verify data checksum
        let actual = crc32fast::hash(&decompressed);
        if actual != self.segment.footer.data_checksum {
            return Err(SegmentError::ChecksumMismatch {
                expected: self.segment.footer.data_checksum,
                actual,
            });
        }

        Ok(())
    }

    /// Get segment header
    pub fn header(&self) -> &SegmentHeader {
        &self.segment.header
    }

    /// Get segment footer
    pub fn footer(&self) -> &SegmentFooter {
        &self.segment.footer
    }

    /// Get the underlying segment
    pub fn segment(&self) -> &Segment {
        &self.segment
    }

    /// Decompress record data if needed
    fn decompress_data(&self) -> Result<Vec<u8>, SegmentError> {
        match self.compression {
            Compression::None => Ok(self.segment.record_data.clone()),
            #[cfg(feature = "compression")]
            Compression::Zstd { .. } => {
                let decompressed = zstd::decode_all(self.segment.record_data.as_slice())?;
                Ok(decompressed)
            }
        }
    }

    /// Iterator over deltas in the segment
    pub fn deltas(&self) -> Result<DeltaIterator, SegmentError> {
        let data = self.decompress_data()?;
        Ok(DeltaIterator {
            data,
            offset: 0,
            remaining: self.segment.header.record_count,
        })
    }

    /// Read all deltas into a vector
    pub fn read_all(&self) -> Result<Vec<ReplicationDelta>, SegmentError> {
        self.deltas()?.collect()
    }
}

/// Iterator over deltas in a segment
pub struct DeltaIterator {
    data: Vec<u8>,
    offset: usize,
    remaining: u32,
}

impl Iterator for DeltaIterator {
    type Item = Result<ReplicationDelta, SegmentError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 || self.offset >= self.data.len() {
            return None;
        }

        // Read length prefix
        if self.offset + 4 > self.data.len() {
            return Some(Err(SegmentError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected end of record data",
            ))));
        }

        // TigerStyle: try_into() is safe - bounds check above ensures 4 bytes available
        let len = u32::from_le_bytes(
            self.data[self.offset..self.offset + 4]
                .try_into()
                .expect("bounds checked: offset + 4 <= data.len()"),
        ) as usize;
        self.offset += 4;

        if self.offset + len > self.data.len() {
            return Some(Err(SegmentError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Record data truncated",
            ))));
        }

        // Deserialize delta
        let result = bincode::deserialize(&self.data[self.offset..self.offset + len]);
        self.offset += len;
        self.remaining -= 1;

        Some(result.map_err(SegmentError::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::SDS;
    use crate::replication::lattice::{LamportClock, ReplicaId};
    use crate::replication::state::ReplicatedValue;

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
    fn test_segment_roundtrip() {
        let mut writer = SegmentWriter::new(Compression::None);

        writer
            .write_delta(&make_delta("key1", "value1", 100))
            .unwrap();
        writer
            .write_delta(&make_delta("key2", "value2", 200))
            .unwrap();
        writer
            .write_delta(&make_delta("key3", "value3", 300))
            .unwrap();

        let data = writer.finish().unwrap();
        let reader = SegmentReader::open(&data).unwrap();

        assert_eq!(reader.header().record_count, 3);
        assert_eq!(reader.header().min_timestamp, 100);
        assert_eq!(reader.header().max_timestamp, 300);

        // Validate checksums
        reader.validate().unwrap();

        // Read all deltas
        let deltas: Vec<_> = reader.read_all().unwrap();
        assert_eq!(deltas.len(), 3);
        assert_eq!(deltas[0].key, "key1");
        assert_eq!(deltas[1].key, "key2");
        assert_eq!(deltas[2].key, "key3");
    }

    #[test]
    fn test_segment_empty_error() {
        let writer = SegmentWriter::new(Compression::None);
        assert!(matches!(writer.finish(), Err(SegmentError::Empty)));
    }

    #[test]
    fn test_segment_invalid_magic() {
        let mut data = vec![0u8; 100];
        data[0..4].copy_from_slice(b"XXXX"); // Wrong magic

        let result = SegmentReader::open(&data);
        assert!(matches!(result, Err(SegmentError::InvalidMagic)));
    }

    #[test]
    fn test_segment_checksum_mismatch() {
        let mut writer = SegmentWriter::new(Compression::None);
        writer
            .write_delta(&make_delta("key1", "value1", 100))
            .unwrap();

        let mut data = writer.finish().unwrap();

        // Corrupt some record data
        let corrupt_offset = HEADER_SIZE + 10;
        if corrupt_offset < data.len() - FOOTER_SIZE {
            data[corrupt_offset] ^= 0xFF;
        }

        let reader = SegmentReader::open(&data).unwrap();
        let result = reader.validate();
        assert!(matches!(result, Err(SegmentError::ChecksumMismatch { .. })));
    }

    #[test]
    fn test_segment_header_validation() {
        let header = SegmentHeader::new(10, 100, 200, Compression::None);
        assert_eq!(header.magic, SEGMENT_MAGIC);
        assert_eq!(header.version, SEGMENT_VERSION);
        header.validate().unwrap();

        // Corrupt checksum
        let mut corrupted = header.clone();
        corrupted.header_checksum = 12345;
        assert!(corrupted.validate().is_err());
    }

    #[test]
    fn test_segment_estimated_size() {
        let mut writer = SegmentWriter::new(Compression::None);
        assert_eq!(writer.estimated_size(), HEADER_SIZE + FOOTER_SIZE);

        writer
            .write_delta(&make_delta("key1", "value1", 100))
            .unwrap();
        assert!(writer.estimated_size() > HEADER_SIZE + FOOTER_SIZE);
    }

    #[test]
    fn test_segment_large() {
        let mut writer = SegmentWriter::new(Compression::None);

        // Write 1000 deltas
        for i in 0..1000 {
            let key = format!("key{:06}", i);
            let value = format!("value{:06}", i);
            writer
                .write_delta(&make_delta(&key, &value, i as u64))
                .unwrap();
        }

        let data = writer.finish().unwrap();
        let reader = SegmentReader::open(&data).unwrap();

        assert_eq!(reader.header().record_count, 1000);
        reader.validate().unwrap();

        let deltas = reader.read_all().unwrap();
        assert_eq!(deltas.len(), 1000);
        assert_eq!(deltas[0].key, "key000000");
        assert_eq!(deltas[999].key, "key000999");
    }

    #[test]
    fn test_header_footer_serialization() {
        let header = SegmentHeader::new(42, 100, 200, Compression::None);
        let bytes = header.to_bytes();
        let parsed = SegmentHeader::from_bytes(&bytes).unwrap();

        assert_eq!(header.magic, parsed.magic);
        assert_eq!(header.version, parsed.version);
        assert_eq!(header.record_count, parsed.record_count);
        assert_eq!(header.min_timestamp, parsed.min_timestamp);
        assert_eq!(header.max_timestamp, parsed.max_timestamp);
        assert_eq!(header.header_checksum, parsed.header_checksum);

        let footer = SegmentFooter::new(12345, 1000, 800);
        let bytes = footer.to_bytes();
        let parsed = SegmentFooter::from_bytes(&bytes).unwrap();

        assert_eq!(footer.data_checksum, parsed.data_checksum);
        assert_eq!(footer.uncompressed_size, parsed.uncompressed_size);
        assert_eq!(footer.compressed_size, parsed.compressed_size);
        assert_eq!(footer.footer_magic, parsed.footer_magic);
    }
}
