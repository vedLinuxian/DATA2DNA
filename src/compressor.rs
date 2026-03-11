//! Module 6: Helix Compressor Engine — Rust Edition
//!
//! Multi-algorithm compression with parallel evaluation via rayon.
//! Tries ZSTD-19, Brotli-11, Deflate-9, LZ4, Dedup+ZSTD in parallel
//! and picks the smallest result. Strips image metadata (JPEG EXIF, PNG chunks).

use flate2::write::ZlibEncoder;
use flate2::read::ZlibDecoder;
use flate2::Compression;
use rayon::prelude::*;
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::io::{Read, Write, Cursor};
use serde::Serialize;

// Format constants
pub const MAGIC: &[u8; 4] = b"HLXR";
pub const FORMAT_VERSION: u8 = 1;

// Compression method IDs
pub const METHOD_NONE: u8 = 0;
pub const METHOD_ZSTD: u8 = 1;
pub const METHOD_BROTLI: u8 = 2;
pub const METHOD_LZ4: u8 = 3;
pub const METHOD_DEFLATE: u8 = 4;
pub const METHOD_DEDUP_ZSTD: u8 = 5;
pub const METHOD_STRIPPED: u8 = 6; // metadata-stripped + inner method

// Header: MAGIC(4) + VERSION(1) + METHOD(1) + ORIG_SIZE(8 LE) = 14 bytes
pub const HEADER_SIZE: usize = 14;

#[derive(Debug, Clone, Serialize, Default)]
pub struct CompressionStats {
    pub original_size: usize,
    pub compressed_size: usize,
    pub method: String,
    pub compression_ratio: f64,
    pub space_saving_percent: f64,
    pub throughput_mbps: f64,
    pub time_seconds: f64,
    pub checksum: String,
    pub saved_bytes: i64,
    pub content_type_detected: String,
    pub compression_note: String,
    pub dedup_savings: usize,
    pub dedup_unique_blocks: usize,
    pub dedup_total_blocks: usize,
    pub stages: Vec<StageInfo>,
    pub all_methods_tried: Vec<MethodResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageInfo {
    pub name: String,
    pub output_size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodResult {
    pub method: String,
    pub size: usize,
}

pub struct HelixCompressor {
    pub zstd_level: i32,
    pub brotli_quality: i32,
    pub block_size: usize,
}

macro_rules! progress {
    ($cb:expr, $phase:expr, $pct:expr) => {
        if let Some(ref f) = $cb {
            f($phase, $pct);
        }
    };
}

impl HelixCompressor {
    pub fn new(level: &str) -> Self {
        let (zstd_level, brotli_quality) = match level {
            "fast" => (3, 4),
            "normal" => (9, 6),
            "high" => (15, 9),
            "ultra" | _ => (19, 11),
        };
        Self {
            zstd_level,
            brotli_quality,
            block_size: 4096,
        }
    }

    pub fn compress(
        &self,
        data: &[u8],
        progress_cb: Option<&dyn Fn(&str, u32)>,
    ) -> (Vec<u8>, CompressionStats) {
        let start = std::time::Instant::now();
        let mut stats = CompressionStats::default();
        stats.original_size = data.len();
        stats.checksum = hex_sha256(data)[..16].to_string();
        stats.content_type_detected = detect_content_type(data).to_string();

        if data.is_empty() {
            return (Vec::new(), stats);
        }

        progress!(progress_cb, "Analyzing data...", 5);

        // Stage 0: Analyze potential metadata stripping opportunity (informational only).
        // We keep compression strictly lossless and always compress the original input bytes.
        let stripped = self.strip_metadata(data);
        let metadata_saved = data.len().saturating_sub(stripped.len());
        let working = data;

        if metadata_saved > 0 {
            stats.stages.push(StageInfo {
                name: "Metadata Strip".into(),
                output_size: stripped.len(),
            });
        }

        progress!(progress_cb, "Compressing (all methods parallel)...", 15);

        let zstd_level = self.zstd_level;
        let brotli_q = self.brotli_quality;
        let block_size = self.block_size;

        // PERF-07: Skip parallel compression for very small data
        let mut candidates: Vec<(String, u8, Vec<u8>)> = if working.len() < 1024 {
            // For small data, only try ZSTD (fastest to initialize)
            let mut c = Vec::new();
            if let Ok(compressed) = zstd::encode_all(Cursor::new(working), zstd_level) {
                c.push(("zstd".to_string(), METHOD_ZSTD, compressed));
            }
            if let Ok(compressed) = compress_deflate(working) {
                c.push(("deflate".to_string(), METHOD_DEFLATE, compressed));
            }
            c
        } else {
            let methods: Vec<(&str, u8)> = vec![
                ("zstd", METHOD_ZSTD),
                ("brotli", METHOD_BROTLI),
                ("deflate", METHOD_DEFLATE),
                ("lz4", METHOD_LZ4),
            ];

            // Parallel compression via rayon
            methods
                .par_iter()
                .filter_map(|&(name, method_id)| {
                    let compressed = match name {
                        "zstd" => zstd::encode_all(Cursor::new(working), zstd_level).ok(),
                        "brotli" => compress_brotli(working, brotli_q).ok(),
                        "deflate" => compress_deflate(working).ok(),
                        "lz4" => Some(lz4_flex::compress_prepend_size(working)),
                        _ => None,
                    };
                    compressed.map(|c| (name.to_string(), method_id, c))
                })
                .collect()
        };

        // Also try dedup+ZSTD for data > 8KB
        if working.len() > 8192 {
            if let Ok(dedup_result) = dedup_compress(working, block_size, zstd_level) {
                let dedup_info = dedup_analyze(working, block_size);
                stats.dedup_savings = dedup_info.0;
                stats.dedup_unique_blocks = dedup_info.1;
                stats.dedup_total_blocks = dedup_info.2;
                candidates.push(("dedup+zstd".into(), METHOD_DEDUP_ZSTD, dedup_result));
            }
        }

        progress!(progress_cb, "Evaluating results...", 70);

        // Record all results for stats
        stats.all_methods_tried = candidates
            .iter()
            .map(|(name, _, c)| MethodResult {
                method: name.clone(),
                size: c.len(),
            })
            .collect();

        progress!(progress_cb, "Selecting best method...", 85);

        // Find the best (smallest) result
        let best = candidates
            .iter()
            .min_by_key(|(_, _, c)| c.len());

        let (method_name, method_id, compressed_data) = match best {
            Some((name, mid, c)) => (name.clone(), *mid, c.clone()),
            None => {
                // Fallback: zlib
                let c = compress_deflate(data).unwrap_or_else(|_| data.to_vec());
                ("deflate_fallback".into(), METHOD_DEFLATE, c)
            }
        };

        // If raw data is smaller, don't compress
        let (final_method_id, final_data, final_method_name) = if compressed_data.len() + HEADER_SIZE >= data.len() {
            (METHOD_NONE, data.to_vec(), "none".to_string())
        } else {
            (method_id, compressed_data, method_name)
        };

        // Build final output: HEADER + DATA
        let output = pack_compressed(final_method_id, data.len(), &final_data);

        progress!(progress_cb, "Complete", 100);

        // Fill stats
        let elapsed = start.elapsed().as_secs_f64();
        stats.compressed_size = output.len();
        stats.method = final_method_name;
        stats.compression_ratio = if output.len() > 0 {
            data.len() as f64 / output.len() as f64
        } else {
            f64::INFINITY
        };
        stats.space_saving_percent =
            (1.0 - output.len() as f64 / data.len() as f64) * 100.0;
        stats.saved_bytes = data.len() as i64 - output.len() as i64;
        stats.throughput_mbps = if elapsed > 0.0 {
            data.len() as f64 / (1024.0 * 1024.0) / elapsed
        } else {
            0.0
        };
        stats.time_seconds = elapsed;
        stats.compression_note = compression_note(
            &stats.content_type_detected,
            stats.space_saving_percent,
        );

        (output, stats)
    }

    pub fn decompress(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if data.len() < HEADER_SIZE {
            anyhow::bail!("Data too short for Helix format");
        }

        // Verify magic
        if &data[0..4] != MAGIC {
            anyhow::bail!("Invalid magic bytes");
        }
        let _version = data[4];
        let method = data[5];
        let orig_size = u64::from_le_bytes(data[6..14].try_into()?) as usize;
        let payload = &data[HEADER_SIZE..];

        let decompressed = match method {
            METHOD_NONE => payload.to_vec(),
            METHOD_ZSTD => zstd::decode_all(Cursor::new(payload))?,
            METHOD_BROTLI => decompress_brotli(payload)?,
            METHOD_LZ4 => lz4_flex::decompress_size_prepended(payload)
                .map_err(|e| anyhow::anyhow!("LZ4 error: {}", e))?,
            METHOD_DEFLATE => decompress_deflate(payload)?,
            METHOD_DEDUP_ZSTD => dedup_decompress(payload)?,
            METHOD_STRIPPED => {
                anyhow::bail!(
                    "Unsupported legacy STRIPPED payload: this format is lossy and cannot restore original bytes"
                )
            }
            _ => anyhow::bail!("Unknown method: {}", method),
        };

        // Verify original size
        if decompressed.len() != orig_size {
            anyhow::bail!(
                "Size mismatch: expected {} got {}",
                orig_size,
                decompressed.len()
            );
        }

        Ok(decompressed)
    }

    // ========== Metadata stripping ==========

    fn strip_metadata(&self, data: &[u8]) -> Vec<u8> {
        if data.len() < 4 {
            return data.to_vec();
        }
        if data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
            return strip_jpeg_metadata(data);
        }
        if data.len() >= 8
            && data[0..4] == [0x89, 0x50, 0x4E, 0x47]
        {
            return strip_png_metadata(data);
        }
        data.to_vec()
    }
}

// ========== Packing / unpacking ==========

fn pack_compressed(method: u8, original_size: usize, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + data.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.push(method);
    out.extend_from_slice(&(original_size as u64).to_le_bytes());
    out.extend_from_slice(data);
    out
}

// ========== Individual compressors ==========

fn compress_brotli(data: &[u8], quality: i32) -> anyhow::Result<Vec<u8>> {
    let mut output = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut output, 4096, quality as u32, 22);
        writer.write_all(data)?;
    }
    Ok(output)
}

fn decompress_brotli(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut reader = brotli::Decompressor::new(Cursor::new(data), 4096);
    reader.read_to_end(&mut output)?;
    Ok(output)
}

fn compress_deflate(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

fn decompress_deflate(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output)?;
    Ok(output)
}

// ========== Block deduplication ==========

fn dedup_analyze(data: &[u8], block_size: usize) -> (usize, usize, usize) {
    let blocks: Vec<&[u8]> = data.chunks(block_size).collect();
    let mut seen: HashMap<[u8; 32], bool> = HashMap::new();
    let mut unique = 0usize;
    let mut duplicate_bytes = 0usize;
    for block in &blocks {
        let hash: [u8; 32] = Sha256::digest(block).into();
        if seen.insert(hash, true).is_none() {
            unique += 1;
        } else {
            duplicate_bytes += block.len();
        }
    }
    let total = blocks.len();
    (duplicate_bytes, unique, total)
}

fn dedup_compress(data: &[u8], block_size: usize, zstd_level: i32) -> anyhow::Result<Vec<u8>> {
    let blocks: Vec<&[u8]> = data.chunks(block_size).collect();
    let mut unique_data: Vec<Vec<u8>> = Vec::new();
    let mut hash_to_idx: HashMap<[u8; 32], u32> = HashMap::new();
    let mut indices: Vec<u32> = Vec::new();

    for block in &blocks {
        let hash: [u8; 32] = Sha256::digest(block).into();
        let idx = *hash_to_idx.entry(hash).or_insert_with(|| {
            unique_data.push(block.to_vec());
            (unique_data.len() - 1) as u32
        });
        indices.push(idx);
    }

    // Serialize: num_unique(4) | num_indices(4) | block_size(4) |
    //            for each unique: len(4) data | indices(4 each)
    // BUG-09 FIX: Use u32 for block lengths to avoid overflow at >65KB blocks
    let mut packed = Vec::new();
    packed.extend_from_slice(&(unique_data.len() as u32).to_le_bytes());
    packed.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    packed.extend_from_slice(&(block_size as u32).to_le_bytes());
    for block in &unique_data {
        packed.extend_from_slice(&(block.len() as u32).to_le_bytes());
        packed.extend_from_slice(block);
    }
    for idx in &indices {
        packed.extend_from_slice(&idx.to_le_bytes());
    }

    // Compress the packed data with ZSTD
    let compressed = zstd::encode_all(Cursor::new(&packed), zstd_level)?;
    Ok(compressed)
}

fn dedup_decompress(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let packed = zstd::decode_all(Cursor::new(data))?;
    let mut pos = 0;

    let read_u32 = |buf: &[u8], pos: &mut usize| -> anyhow::Result<u32> {
        if *pos + 4 > buf.len() {
            anyhow::bail!("Malformed dedup payload: unexpected end while reading u32");
        }
        let out = u32::from_le_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]);
        *pos += 4;
        Ok(out)
    };

    let num_unique = read_u32(&packed, &mut pos)? as usize;
    let num_indices = read_u32(&packed, &mut pos)? as usize;
    let _block_size = read_u32(&packed, &mut pos)? as usize;

    let mut unique_blocks: Vec<Vec<u8>> = Vec::with_capacity(num_unique);
    for _ in 0..num_unique {
        // BUG-09 FIX: Read u32 instead of u16 for block lengths
        let len = read_u32(&packed, &mut pos)? as usize;
        if pos + len > packed.len() {
            anyhow::bail!("Malformed dedup payload: unique block exceeds payload length");
        }
        unique_blocks.push(packed[pos..pos + len].to_vec());
        pos += len;
    }

    let mut result = Vec::new();
    for _ in 0..num_indices {
        let idx = read_u32(&packed, &mut pos)? as usize;
        if idx >= unique_blocks.len() {
            anyhow::bail!("Malformed dedup payload: index out of range ({idx})");
        }
        result.extend_from_slice(&unique_blocks[idx]);
    }

    if pos != packed.len() {
        anyhow::bail!("Malformed dedup payload: trailing bytes detected");
    }

    Ok(result)
}

// ========== Image metadata stripping ==========

fn strip_jpeg_metadata(data: &[u8]) -> Vec<u8> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return data.to_vec();
    }

    let mut result = vec![0xFF, 0xD8]; // SOI
    let mut pos = 2;

    while pos + 1 < data.len() {
        if data[pos] != 0xFF {
            result.extend_from_slice(&data[pos..]);
            break;
        }

        let marker = data[pos + 1];

        // EOI
        if marker == 0xD9 {
            result.extend_from_slice(&[0xFF, 0xD9]);
            break;
        }

        // SOS — copy everything from here to end (scan data + EOI)
        if marker == 0xDA {
            result.extend_from_slice(&data[pos..]);
            break;
        }

        // Restart markers (no length field)
        if (0xD0..=0xD7).contains(&marker) || marker == 0x00 || marker == 0x01 {
            result.push(data[pos]);
            pos += 1;
            if marker != 0xFF {
                result.push(data[pos]);
                pos += 1;
            }
            continue;
        }

        // Marker with length
        if pos + 3 >= data.len() {
            result.extend_from_slice(&data[pos..]);
            break;
        }

        let length =
            ((data[pos + 2] as usize) << 8) | (data[pos + 3] as usize);
        let segment_end = (pos + 2 + length).min(data.len());

        // Strip: APP0-APP15 (E0-EF), COM (FE) — these contain EXIF, XMP, ICC, etc.
        if (marker >= 0xE0 && marker <= 0xEF) || marker == 0xFE {
            pos = segment_end;
            continue;
        }

        // Keep this segment (DQT, DHT, SOF, DRI, etc.)
        result.extend_from_slice(&data[pos..segment_end]);
        pos = segment_end;
    }

    result
}

fn strip_png_metadata(data: &[u8]) -> Vec<u8> {
    const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    const KEEP: &[&[u8; 4]] =
        &[b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"pHYs"];

    if data.len() < 8 || data[..8] != PNG_SIG {
        return data.to_vec();
    }

    let mut result = Vec::from(&PNG_SIG[..]);
    let mut pos = 8;

    while pos + 12 <= data.len() {
        let length =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let chunk_end = pos + 12 + length; // 4(len) + 4(type) + data + 4(crc)

        if chunk_end > data.len() {
            result.extend_from_slice(&data[pos..]);
            break;
        }

        let keep = KEEP.iter().any(|ct| chunk_type == &ct[..]);
        if keep {
            result.extend_from_slice(&data[pos..chunk_end]);
        }

        pos = chunk_end;

        if chunk_type == b"IEND" {
            break;
        }
    }

    result
}

// ========== Utility ==========

fn detect_content_type(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "application/octet-stream";
    }
    if data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return "image/jpeg";
    }
    if data[..4] == [0x89, 0x50, 0x4E, 0x47] {
        return "image/png";
    }
    if data[..3] == [0x47, 0x49, 0x46] {
        return "image/gif";
    }
    if data[..4] == [0x50, 0x4B, 0x03, 0x04] {
        return "application/zip";
    }
    if data[..2] == [0x1F, 0x8B] {
        return "application/gzip";
    }
    if data.starts_with(b"%PDF") {
        return "application/pdf";
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return "image/webp";
    }
    // Check if text-like
    let sample = &data[..data.len().min(512)];
    let text_count = sample
        .iter()
        .filter(|&&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
        .count();
    if text_count as f64 / sample.len() as f64 > 0.85 {
        return "text/plain";
    }
    "application/octet-stream"
}

fn compression_note(content_type: &str, saving_pct: f64) -> String {
    if saving_pct < 5.0 && (content_type.starts_with("image/") || content_type == "application/zip" || content_type == "application/gzip") {
        format!(
            "{} files are already compressed internally. {:.1}% savings is the physical maximum \
             (Shannon entropy limit). Try text/CSV/JSON for 90-99% compression.",
            content_type, saving_pct.max(0.0)
        )
    } else if saving_pct >= 90.0 {
        format!("Excellent compression: {:.1}% space saved!", saving_pct)
    } else if saving_pct >= 50.0 {
        format!("Good compression: {:.1}% space saved.", saving_pct)
    } else {
        String::new()
    }
}

pub fn hex_sha256(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_text() {
        let compressor = HelixCompressor::new("ultra");
        let data = b"Hello, Helix-Core Rust Edition! DNA data storage is the future. ".repeat(1000);
        let (compressed, stats) = compressor.compress(&data, None);
        assert!(compressed.len() < data.len());
        assert!(stats.compression_ratio > 1.0);
        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_roundtrip_binary() {
        let compressor = HelixCompressor::new("ultra");
        let data: Vec<u8> = (0..50000).map(|i| (i % 256) as u8).collect();
        let (compressed, _stats) = compressor.compress(&data, None);
        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_tiny_data() {
        let compressor = HelixCompressor::new("ultra");
        let data = b"Hi";
        let (compressed, _) = compressor.compress(data, None);
        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_highly_repetitive() {
        let compressor = HelixCompressor::new("ultra");
        let data = vec![0u8; 500_000];
        let (compressed, stats) = compressor.compress(&data, None);
        println!(
            "500KB zeros: {} -> {} bytes ({:.0}x)",
            data.len(),
            compressed.len(),
            stats.compression_ratio
        );
        assert!(stats.compression_ratio > 100.0);
        let decompressed = compressor.decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }
}
