//! One-shot reader for recorded results in a legacy SQLite `chisel.db`.
//! The live snapshot stays `.chisel/store`. This module never writes SQLite.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct LegacyResult {
    pub test_id: String,
    pub passed: bool,
    pub duration_ms: Option<f64>,
    pub recorded_at: f64,
}

struct Db {
    bytes: Vec<u8>,
    page_size: usize,
    reserved: usize,
}

impl Db {
    fn open(path: &Path) -> Result<Db, String> {
        let mut bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if bytes.len() < 100 || &bytes[..16] != b"SQLite format 3\0" {
            return Err("not a SQLite database".into());
        }
        apply_wal(path, &mut bytes);
        let page_size = page_size_of(&bytes)?;
        let reserved = bytes[20] as usize;
        Ok(Db { bytes, page_size, reserved })
    }

    /// Root page and column names of `table`.
    fn table(&self, table: &str) -> Result<(usize, Vec<String>), String> {
        for row in table_rows(&self.bytes, self.page_size, self.reserved, 1)? {
            let typ = row.first().and_then(Cell::as_text).unwrap_or("");
            let name = row.get(1).and_then(Cell::as_text).unwrap_or("");
            if typ == "table" && name == table {
                let root = row.get(3).and_then(Cell::as_int).unwrap_or(0);
                if root < 1 {
                    return Err(format!("{table} has no root page"));
                }
                let columns = row.get(4).and_then(Cell::as_text).map(column_names).unwrap_or_default();
                return Ok((root as usize, columns));
            }
        }
        Err(format!("{table} table not found"))
    }

    fn rows(&self, root: usize) -> Result<Vec<Vec<Cell>>, String> {
        table_rows(&self.bytes, self.page_size, self.reserved, root)
    }
}

/// Number of rows in `table` of a SQLite file, read without SQLite.
pub fn table_row_count(path: &Path, table: &str) -> Result<usize, String> {
    let db = Db::open(path)?;
    let (root, _) = db.table(table)?;
    Ok(db.rows(root)?.len())
}

pub fn read_test_results(path: &Path) -> Result<Vec<LegacyResult>, String> {
    let db = Db::open(path)?;
    let (root, columns) = db.table("test_results")?;
    let idx = |name: &str, fallback: usize| -> usize {
        columns.iter().position(|c| c == name).unwrap_or(fallback)
    };
    let id_i = idx("test_id", 0);
    let passed_i = idx("passed", 1);
    let dur_i = idx("duration_ms", 2);
    let at_i = idx("recorded_at", 3);
    let mut out = Vec::new();
    for row in db.rows(root)? {
        let Some(test_id) = row.get(id_i).and_then(Cell::as_text) else { continue };
        if test_id.is_empty() {
            continue;
        }
        let passed = row.get(passed_i).and_then(Cell::as_int).unwrap_or(0) != 0;
        let duration_ms = row.get(dur_i).and_then(Cell::as_number);
        let recorded_at = match row.get(at_i) {
            Some(Cell::Text(s)) => parse_recorded_at(s).unwrap_or(0.0),
            Some(Cell::Int(n)) => *n as f64,
            Some(Cell::Float(n)) => *n,
            _ => 0.0,
        };
        out.push(LegacyResult {
            test_id: test_id.to_string(),
            passed,
            duration_ms,
            recorded_at,
        });
    }
    out.sort_by(|a, b| a.recorded_at.partial_cmp(&b.recorded_at).unwrap_or(std::cmp::Ordering::Equal).then(a.test_id.cmp(&b.test_id)));
    Ok(out)
}

fn page_size_of(bytes: &[u8]) -> Result<usize, String> {
    let raw = u16::from_be_bytes([bytes[16], bytes[17]]);
    let size = if raw == 1 { 65536 } else { raw as usize };
    if size < 512 || !size.is_power_of_two() {
        return Err(format!("bad page size {size}"));
    }
    Ok(size)
}

#[derive(Clone, Debug)]
enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Blob,
}

impl Cell {
    fn as_text(&self) -> Option<&str> {
        match self {
            Cell::Text(s) => Some(s),
            _ => None,
        }
    }
    fn as_int(&self) -> Option<i64> {
        match self {
            Cell::Int(n) => Some(*n),
            Cell::Float(n) => Some(*n as i64),
            _ => None,
        }
    }
    fn as_number(&self) -> Option<f64> {
        match self {
            Cell::Int(n) => Some(*n as f64),
            Cell::Float(n) => Some(*n),
            _ => None,
        }
    }
}

fn table_rows(bytes: &[u8], page_size: usize, reserved: usize, root: usize) -> Result<Vec<Vec<Cell>>, String> {
    let mut rows = Vec::new();
    let mut stack = vec![root];
    let mut guard = 0;
    while let Some(page_no) = stack.pop() {
        guard += 1;
        if guard > 100_000 {
            return Err("sqlite page walk too deep".into());
        }
        let page = page_bytes(bytes, page_size, page_no)?;
        let header_at = if page_no == 1 { 100 } else { 0 };
        if header_at >= page.len() {
            continue;
        }
        let kind = page[header_at];
        let cell_count = u16::from_be_bytes([page[header_at + 3], page[header_at + 4]]) as usize;
        let interior = kind == 0x05;
        let ptr_at = header_at + if interior { 12 } else { 8 };
        if interior {
            let right = u32::from_be_bytes([page[header_at + 8], page[header_at + 9], page[header_at + 10], page[header_at + 11]]) as usize;
            if right > 0 {
                stack.push(right);
            }
        }
        if kind != 0x0d && kind != 0x05 {
            continue;
        }
        for n in 0..cell_count {
            let at = ptr_at + n * 2;
            if at + 2 > page.len() {
                break;
            }
            let cell_off = u16::from_be_bytes([page[at], page[at + 1]]) as usize;
            if interior {
                if let Some(child) = interior_child(page, cell_off) {
                    stack.push(child);
                }
            } else if let Some(payload) = leaf_payload(bytes, page, page_size, reserved, cell_off) {
                rows.push(decode_record(&payload));
            }
        }
    }
    Ok(rows)
}

fn page_bytes(bytes: &[u8], page_size: usize, page_no: usize) -> Result<&[u8], String> {
    if page_no == 0 {
        return Err("sqlite page 0".into());
    }
    let start = (page_no - 1) * page_size;
    let end = start + page_size;
    if end > bytes.len() {
        return Err(format!("sqlite page {page_no} is past the end of the file"));
    }
    Ok(&bytes[start..end])
}

fn interior_child(page: &[u8], off: usize) -> Option<usize> {
    if off + 4 > page.len() {
        return None;
    }
    Some(u32::from_be_bytes([page[off], page[off + 1], page[off + 2], page[off + 3]]) as usize)
}

fn leaf_payload(file: &[u8], page: &[u8], page_size: usize, reserved: usize, off: usize) -> Option<Vec<u8>> {
    let (payload_size, next) = read_varint(page, off)?;
    let (_rowid, next) = read_varint(page, next)?;
    let payload_size = payload_size as usize;
    let local = local_payload(page_size, reserved, payload_size);
    if next + local > page.len() {
        return None;
    }
    let mut payload = page[next..next + local].to_vec();
    if payload.len() < payload_size {
        if next + local + 4 > page.len() {
            return None;
        }
        let mut overflow = u32::from_be_bytes([
            page[next + local],
            page[next + local + 1],
            page[next + local + 2],
            page[next + local + 3],
        ]) as usize;
        let usable = page_size.saturating_sub(reserved);
        while payload.len() < payload_size && overflow > 0 {
            let ov = page_bytes(file, page_size, overflow).ok()?;
            if ov.len() < 4 {
                break;
            }
            let take = (payload_size - payload.len()).min(usable.saturating_sub(4));
            payload.extend_from_slice(&ov[4..4 + take]);
            overflow = u32::from_be_bytes([ov[0], ov[1], ov[2], ov[3]]) as usize;
        }
    }
    payload.truncate(payload_size);
    Some(payload)
}

fn local_payload(page_size: usize, reserved: usize, payload: usize) -> usize {
    let usable = page_size.saturating_sub(reserved).max(1);
    let max_local = usable.saturating_sub(35);
    if payload <= max_local {
        return payload;
    }
    let min_local = ((usable.saturating_sub(12)) * 32 / 255).saturating_sub(23);
    let mut local = min_local + ((payload - min_local) % usable.saturating_sub(4).max(1));
    if local > max_local {
        local = min_local;
    }
    local.min(payload)
}

fn decode_record(payload: &[u8]) -> Vec<Cell> {
    let Some((header_size, header_start)) = read_varint(payload, 0) else {
        return Vec::new();
    };
    let header_size = header_size as usize;
    if header_size > payload.len() || header_start > header_size {
        return Vec::new();
    }
    let mut types = Vec::new();
    let mut i = header_start;
    while i < header_size {
        let Some((serial, next)) = read_varint(payload, i) else { break };
        types.push(serial);
        i = next;
    }
    let mut body = header_size;
    let mut cells = Vec::new();
    for serial in types {
        let (cell, used) = read_serial(payload, body, serial);
        cells.push(cell);
        body += used;
    }
    cells
}

fn read_serial(payload: &[u8], at: usize, serial: u64) -> (Cell, usize) {
    let take = |n: usize| -> Option<&[u8]> {
        payload.get(at..at + n)
    };
    match serial {
        0 => (Cell::Null, 0),
        1 => (Cell::Int(take(1).map(|b| b[0] as i8 as i64).unwrap_or(0)), 1),
        2 => (Cell::Int(take(2).map(|b| i16::from_be_bytes([b[0], b[1]]) as i64).unwrap_or(0)), 2),
        3 => {
            let b = take(3).unwrap_or(&[0, 0, 0]);
            let mut raw = [0u8; 4];
            raw[1..].copy_from_slice(&b[..3.min(b.len())]);
            let mut n = i32::from_be_bytes(raw) >> 8;
            if b.len() == 3 && b[0] & 0x80 != 0 {
                n |= !0 << 24;
            }
            (Cell::Int(n as i64), 3)
        }
        4 => (Cell::Int(take(4).map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64).unwrap_or(0)), 4),
        5 => {
            let b = take(6).unwrap_or(&[0; 6]);
            let mut raw = [0u8; 8];
            if b.len() == 6 {
                raw[2..].copy_from_slice(b);
            }
            let mut n = i64::from_be_bytes(raw) >> 16;
            if b.len() == 6 && b[0] & 0x80 != 0 {
                n |= !0 << 48;
            }
            (Cell::Int(n), 6)
        }
        6 => (Cell::Int(take(8).map(|b| i64::from_be_bytes(b.try_into().unwrap_or([0; 8]))).unwrap_or(0)), 8),
        7 => (Cell::Float(take(8).map(|b| f64::from_be_bytes(b.try_into().unwrap_or([0; 8]))).unwrap_or(0.0)), 8),
        8 => (Cell::Int(0), 0),
        9 => (Cell::Int(1), 0),
        n if n >= 12 && n % 2 == 0 => {
            let len = ((n - 12) / 2) as usize;
            (Cell::Blob, len)
        }
        n if n >= 13 && n % 2 == 1 => {
            let len = ((n - 13) / 2) as usize;
            let text = payload.get(at..at + len).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            (Cell::Text(text), len)
        }
        _ => (Cell::Null, 0),
    }
}

fn read_varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for n in 0..9 {
        let b = *data.get(i)?;
        i += 1;
        if n == 8 {
            value = (value << 8) | b as u64;
            return Some((value, i));
        }
        value = (value << 7) | (b & 0x7f) as u64;
        if b & 0x80 == 0 {
            return Some((value, i));
        }
    }
    None
}

fn column_names(sql: &str) -> Vec<String> {
    let Some(start) = sql.find('(') else { return Vec::new() };
    let body = &sql[start + 1..];
    let Some(end) = body.rfind(')') else { return Vec::new() };
    let mut names = Vec::new();
    for part in body[..end].split(',') {
        let part = part.trim();
        if part.is_empty() || part.to_ascii_lowercase().starts_with("primary ") || part.to_ascii_lowercase().starts_with("unique ") || part.to_ascii_lowercase().starts_with("constraint ") {
            continue;
        }
        let name = part.split_whitespace().next().unwrap_or("").trim_matches(|c| c == '"' || c == '\'' || c == '`');
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    names
}

pub fn parse_recorded_at(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(n) = text.parse::<f64>() {
        if n > 1_000_000_000.0 {
            return Some(n);
        }
    }
    let (date, rest) = text.split_once('T').unwrap_or((text, "00:00:00"));
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let rest = rest.trim_end_matches('Z');
    let (time, offset) = if let Some(i) = rest.rfind('+') {
        (&rest[..i], Some(&rest[i..]))
    } else if let Some(i) = rest.rfind('-') {
        if i > 0 { (&rest[..i], Some(&rest[i..])) } else { (rest, None) }
    } else {
        (rest, None)
    };
    let mut time_parts = time.split(':');
    let hour: u32 = time_parts.next()?.parse().ok()?;
    let min: u32 = time_parts.next().unwrap_or("0").parse().ok()?;
    let sec_part = time_parts.next().unwrap_or("0");
    let (sec_txt, frac_txt) = sec_part.split_once('.').unwrap_or((sec_part, "0"));
    let sec: u32 = sec_txt.parse().ok()?;
    let frac: f64 = format!("0.{frac_txt}").parse().unwrap_or(0.0);
    let mut unix = civil_unix(year, month, day, hour, min, sec)? as f64 + frac;
    if let Some(off) = offset {
        let sign = if off.starts_with('-') { -1.0 } else { 1.0 };
        let off = off.trim_start_matches(['+', '-']);
        let mut p = off.split(':');
        let oh: f64 = p.next()?.parse().ok()?;
        let om: f64 = p.next().unwrap_or("0").parse().unwrap_or(0.0);
        unix -= sign * (oh * 3600.0 + om * 60.0);
    }
    Some(unix)
}

fn civil_unix(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era as i64 * 146097 + doe as i64 - 719468;
    Some(days * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64)
}

fn apply_wal(db_path: &Path, bytes: &mut Vec<u8>) {
    let wal_path = wal_sibling(db_path);
    let Ok(wal) = fs::read(&wal_path) else { return };
    if wal.len() < 32 {
        return;
    }
    let magic = u32::from_be_bytes([wal[0], wal[1], wal[2], wal[3]]);
    if magic != 0x377f_0682 && magic != 0x377f_0683 {
        return;
    }
    let page_size = u32::from_be_bytes([wal[8], wal[9], wal[10], wal[11]]) as usize;
    if page_size < 512 || wal.len() < 32 + 24 + page_size {
        return;
    }
    let salt1 = u32::from_be_bytes([wal[16], wal[17], wal[18], wal[19]]);
    let salt2 = u32::from_be_bytes([wal[20], wal[21], wal[22], wal[23]]);
    let frame = 24 + page_size;
    let mut latest: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut committed: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut off = 32;
    while off + frame <= wal.len() {
        let page_no = u32::from_be_bytes([wal[off], wal[off + 1], wal[off + 2], wal[off + 3]]) as usize;
        let db_size = u32::from_be_bytes([wal[off + 4], wal[off + 5], wal[off + 6], wal[off + 7]]);
        let s1 = u32::from_be_bytes([wal[off + 8], wal[off + 9], wal[off + 10], wal[off + 11]]);
        let s2 = u32::from_be_bytes([wal[off + 12], wal[off + 13], wal[off + 14], wal[off + 15]]);
        if s1 != salt1 || s2 != salt2 || page_no == 0 {
            break;
        }
        let page = wal[off + 24..off + 24 + page_size].to_vec();
        latest.insert(page_no, page);
        if db_size > 0 {
            let need = db_size as usize * page_size;
            if bytes.len() < need {
                bytes.resize(need, 0);
            }
            committed.clone_from(&latest);
        }
        off += frame;
    }
    for (page_no, page) in committed {
        let start = (page_no - 1) * page_size;
        let end = start + page_size;
        if end <= bytes.len() && page.len() == page_size {
            bytes[start..end].copy_from_slice(&page);
        }
    }
}

fn wal_sibling(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push("-wal");
    std::path::PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_legacy_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy/chisel.sqlite");
        let rows = read_test_results(&path).unwrap();
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].test_id, "tests/test_calc.py::test_add");
        assert!(rows[0].passed);
        assert_eq!(rows[0].duration_ms, Some(12.0));
        assert!(rows[0].recorded_at > 1_700_000_000.0);
        let sub = rows.iter().find(|r| r.test_id.ends_with("test_sub")).unwrap();
        assert!(!sub.passed);
        assert!(sub.duration_ms.is_none());
    }

    #[test]
    fn counts_rows_of_a_named_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy/chisel.sqlite");
        assert_eq!(table_row_count(&path, "test_results"), Ok(3));
        assert_eq!(table_row_count(&path, "test_units"), Err("test_units table not found".to_string()));
    }

    #[test]
    fn parses_iso_timestamps() {
        let unix = parse_recorded_at("2026-07-01T00:00:00+00:00").unwrap();
        assert!((unix - 1_782_864_000.0).abs() < 1.0, "{unix}");
    }
}
