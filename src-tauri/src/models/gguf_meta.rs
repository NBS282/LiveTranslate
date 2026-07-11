//! Minimal GGUF header reader used for pre-load validation.
//!
//! This deliberately does *not* pull in a full GGUF-parsing crate — it reads
//! just the file header and walks the metadata key-value section (GGUF v2/v3
//! layout) far enough to find `general.architecture`, then stops. Tensor info
//! and tensor data (the bulk of the file) are never read.
//!
//! Layout (all integers little-endian):
//! ```text
//! magic:      [u8; 4]   == b"GGUF"
//! version:    u32
//! tensor_count:  u64
//! kv_count:      u64
//! kv[kv_count]:  { key: gguf_string, value_type: u32, value: <type-dependent> }
//! ```
//! A `gguf_string` is a `u64` length prefix followed by that many UTF-8 bytes
//! (not NUL-terminated).
//!
//! Array-typed values are not parsed: their layout (`element_type: u32, len:
//! u64, elements...`) is variable enough that a partial implementation risks
//! silently misreading the rest of the file. If an array is encountered before
//! `general.architecture` is found, this bails out with `Ok(None)` rather than
//! guessing.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const GGUF_MAGIC: &[u8; 4] = b"GGUF";

/// GGUF metadata value type tags (subset needed to skip or read scalar/string
/// values). See the module docs for why `Array` is a deliberate dead end here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    Bool,
    String,
    Array,
    U64,
    I64,
    F64,
}

impl ValueType {
    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => ValueType::U8,
            1 => ValueType::I8,
            2 => ValueType::U16,
            3 => ValueType::I16,
            4 => ValueType::U32,
            5 => ValueType::I32,
            6 => ValueType::F32,
            7 => ValueType::Bool,
            8 => ValueType::String,
            9 => ValueType::Array,
            10 => ValueType::U64,
            11 => ValueType::I64,
            12 => ValueType::F64,
            _ => return None,
        })
    }

    /// Byte width of a scalar (non-string, non-array) value of this type.
    fn scalar_width(self) -> Option<usize> {
        match self {
            ValueType::U8 | ValueType::I8 | ValueType::Bool => Some(1),
            ValueType::U16 | ValueType::I16 => Some(2),
            ValueType::U32 | ValueType::I32 | ValueType::F32 => Some(4),
            ValueType::U64 | ValueType::I64 | ValueType::F64 => Some(8),
            ValueType::String | ValueType::Array => None,
        }
    }
}

fn read_u32<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(r: &mut R) -> std::io::Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_gguf_string<R: Read>(r: &mut R) -> std::io::Result<String> {
    let len = read_u64(r)?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn skip_bytes<R: Read>(r: &mut R, n: usize) -> std::io::Result<()> {
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)
}

/// Reads just enough of a GGUF file's header and metadata key-value section to
/// find the `general.architecture` string, without loading tensor data.
///
/// Returns `Ok(None)` when the file isn't a recognizable GGUF v2/v3 file, the
/// key is absent, or an array-typed value is encountered before the key is
/// found (see module docs — arrays are intentionally not parsed here).
pub fn read_architecture(path: &Path) -> Result<Option<String>, String> {
    let file = File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut r = BufReader::new(file);

    let mut magic = [0u8; 4];
    if r.read_exact(&mut magic).is_err() {
        return Ok(None);
    }
    if &magic != GGUF_MAGIC {
        return Ok(None);
    }

    let Ok(version) = read_u32(&mut r) else {
        return Ok(None);
    };
    if version < 2 {
        // v1 used a 32-bit tensor count; no v1 GGUF models are in our catalog.
        return Ok(None);
    }

    let Ok(_tensor_count) = read_u64(&mut r) else {
        return Ok(None);
    };
    let Ok(kv_count) = read_u64(&mut r) else {
        return Ok(None);
    };

    for _ in 0..kv_count {
        let Ok(key) = read_gguf_string(&mut r) else {
            return Ok(None);
        };
        let Ok(raw_type) = read_u32(&mut r) else {
            return Ok(None);
        };
        let Some(value_type) = ValueType::from_u32(raw_type) else {
            return Ok(None);
        };

        match value_type {
            ValueType::Array => {
                // Conservative: bail rather than misparse the array layout.
                return Ok(None);
            }
            ValueType::String => {
                let Ok(value) = read_gguf_string(&mut r) else {
                    return Ok(None);
                };
                if key == "general.architecture" {
                    return Ok(Some(value));
                }
            }
            other => {
                let width = other
                    .scalar_width()
                    .expect("non-string, non-array type has a fixed scalar width");
                if skip_bytes(&mut r, width).is_err() {
                    return Ok(None);
                }
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u32_le(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn write_u64_le(buf: &mut Vec<u8>, v: u64) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn write_gguf_string(buf: &mut Vec<u8>, s: &str) {
        write_u64_le(buf, s.len() as u64);
        buf.extend_from_slice(s.as_bytes());
    }

    /// Writes `bytes` to a uniquely-named file under the OS temp dir and
    /// returns its path. Callers remove the file when done.
    fn write_fixture(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lt_gguf_meta_test_{name}_{}_{}.gguf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, bytes).expect("failed to write GGUF test fixture");
        path
    }

    #[test]
    fn reads_architecture_from_handcrafted_fixture() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        write_u32_le(&mut buf, 3); // version
        write_u64_le(&mut buf, 0); // tensor_count
        write_u64_le(&mut buf, 1); // kv_count
        write_gguf_string(&mut buf, "general.architecture");
        write_u32_le(&mut buf, 8); // value_type = String
        write_gguf_string(&mut buf, "parakeet");

        let path = write_fixture("basic", &buf);
        let arch = read_architecture(&path).unwrap();
        assert_eq!(arch.as_deref(), Some("parakeet"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn returns_none_when_key_absent() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        write_u32_le(&mut buf, 3);
        write_u64_le(&mut buf, 0);
        write_u64_le(&mut buf, 1);
        write_gguf_string(&mut buf, "general.name");
        write_u32_le(&mut buf, 8);
        write_gguf_string(&mut buf, "some-model");

        let path = write_fixture("no_arch", &buf);
        let arch = read_architecture(&path).unwrap();
        assert_eq!(arch, None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn returns_none_for_bad_magic() {
        let path = write_fixture("bad_magic", b"NOPE1234");
        let arch = read_architecture(&path).unwrap();
        assert_eq!(arch, None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bails_on_array_type_before_key_found() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        write_u32_le(&mut buf, 3);
        write_u64_le(&mut buf, 0);
        write_u64_le(&mut buf, 2); // 2 KVs: an array first, then a real key
        write_gguf_string(&mut buf, "some.array");
        write_u32_le(&mut buf, 9); // Array type — parser must bail here
                                   // No valid array payload follows; the parser must stop before reading it.

        let path = write_fixture("array_bail", &buf);
        let arch = read_architecture(&path).unwrap();
        assert_eq!(arch, None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skips_scalar_values_of_various_widths() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GGUF");
        write_u32_le(&mut buf, 3);
        write_u64_le(&mut buf, 0);
        write_u64_le(&mut buf, 3); // u32, u64, then the string we want
        write_gguf_string(&mut buf, "some.count");
        write_u32_le(&mut buf, 4); // U32
        write_u32_le(&mut buf, 42);
        write_gguf_string(&mut buf, "some.big_count");
        write_u32_le(&mut buf, 10); // U64
        write_u64_le(&mut buf, 1_000_000);
        write_gguf_string(&mut buf, "general.architecture");
        write_u32_le(&mut buf, 8); // String
        write_gguf_string(&mut buf, "canary");

        let path = write_fixture("skip_scalars", &buf);
        let arch = read_architecture(&path).unwrap();
        assert_eq!(arch.as_deref(), Some("canary"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn returns_none_for_truncated_file() {
        let path = write_fixture("truncated", b"GGUF");
        let arch = read_architecture(&path).unwrap();
        assert_eq!(arch, None);
        let _ = std::fs::remove_file(&path);
    }
}
