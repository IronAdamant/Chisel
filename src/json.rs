//! First-party JSON for tool results, MCP frames, and the snapshot store.
//! No serde. Duplicate object keys keep the last value, matching Python's json module.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => {
                let ma = BTreeMap::from_iter(a.iter().map(|(k, v)| (k, v)));
                let mb = BTreeMap::from_iter(b.iter().map(|(k, v)| (k, v)));
                ma == mb
            }
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
                *b == (*a as f64) && b.fract() == 0.0
            }
            _ => false,
        }
    }
}

#[derive(Debug)]
pub struct Error {
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Value {
    pub fn object(pairs: Vec<(String, Value)>) -> Self {
        Value::Object(pairs)
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Vec<(String, Value)>> {
        match self {
            Value::Object(p) => Some(p),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        let pairs = self.as_object_mut()?;
        pairs.iter_mut().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn insert(&mut self, key: &str, value: Value) {
        if let Some(slot) = self.get_mut(key) {
            *slot = value;
        } else if let Some(pairs) = self.as_object_mut() {
            pairs.push((key.to_string(), value));
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 => {
                Some(*f as i64)
            }
            _ => None,
        }
    }
}

pub fn parse(input: &str) -> Result<Value, Error> {
    if input.len() > 32 * 1024 * 1024 {
        return Err(Error { message: "json exceeds 32MiB".into() });
    }
    let mut p = Parser {
        bytes: input.as_bytes(),
        i: 0,
        depth: 0,
    };
    p.skip_ws();
    if p.eof() {
        return Err(Error { message: "empty json".into() });
    }
    let v = p.parse_value()?;
    p.skip_ws();
    if !p.eof() {
        return Err(Error { message: "trailing data".into() });
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    i: usize,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn eof(&self) -> bool {
        self.i >= self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.i += 1;
        }
    }

    fn err(&self, message: &str) -> Error {
        Error { message: message.into() }
    }

    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(self.err("json nesting too deep"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn parse_value(&mut self) -> Result<Value, Error> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.consume_ident(b"null").map(|_| Value::Null),
            Some(b't') => self.consume_ident(b"true").map(|_| Value::Bool(true)),
            Some(b'f') => self.consume_ident(b"false").map(|_| Value::Bool(false)),
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(b'N') | Some(b'I') => Err(self.err("NaN and Infinity are rejected")),
            _ => Err(self.err("invalid json value")),
        }
    }

    fn consume_ident(&mut self, lit: &[u8]) -> Result<(), Error> {
        for &b in lit {
            if self.bump() != Some(b) {
                return Err(self.err("invalid literal"));
            }
        }
        Ok(())
    }

    fn parse_array(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.bump();
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.bump();
            self.leave();
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.bump() {
                Some(b']') => break,
                Some(b',') => {
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        self.leave();
                        return Err(self.err("trailing comma"));
                    }
                }
                _ => {
                    self.leave();
                    return Err(self.err("expected comma or end of array"));
                }
            }
        }
        self.leave();
        Ok(Value::Array(items))
    }

    fn parse_object(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.bump();
        let mut pairs: Vec<(String, Value)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.bump();
            self.leave();
            return Ok(Value::Object(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                self.leave();
                return Err(self.err("object key must be a string"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bump() != Some(b':') {
                self.leave();
                return Err(self.err("expected colon"));
            }
            let value = self.parse_value()?;
            if let Some(slot) = pairs.iter_mut().rev().find(|(k, _)| k == &key) {
                slot.1 = value;
            } else {
                pairs.push((key, value));
            }
            self.skip_ws();
            match self.bump() {
                Some(b'}') => break,
                Some(b',') => {
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        self.leave();
                        return Err(self.err("trailing comma"));
                    }
                }
                _ => {
                    self.leave();
                    return Err(self.err("expected comma or end of object"));
                }
            }
        }
        self.leave();
        Ok(Value::Object(pairs))
    }

    fn parse_string(&mut self) -> Result<String, Error> {
        if self.bump() != Some(b'"') {
            return Err(self.err("expected string"));
        }
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => return Ok(out),
                Some(b'\\') => {
                    let esc = self.bump().ok_or_else(|| self.err("bad escape"))?;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut hex = [0u8; 4];
                            for slot in &mut hex {
                                *slot = self.bump().ok_or_else(|| self.err("bad unicode escape"))?;
                            }
                            let s = std::str::from_utf8(&hex).map_err(|_| self.err("bad unicode escape"))?;
                            let cp = u32::from_str_radix(s, 16).map_err(|_| self.err("bad unicode escape"))?;
                            let ch = char::from_u32(cp).ok_or_else(|| self.err("bad unicode scalar"))?;
                            out.push(ch);
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                Some(b) if b < 0x20 => return Err(self.err("raw control character in string")),
                Some(b) => {
                    // Decode one UTF-8 scalar starting at the byte we already consumed.
                    let start = self.i - 1;
                    let width = utf8_width(b).ok_or_else(|| self.err("bad utf-8"))?;
                    if start + width > self.bytes.len() {
                        return Err(self.err("bad utf-8"));
                    }
                    let slice = &self.bytes[start..start + width];
                    let s = std::str::from_utf8(slice).map_err(|_| self.err("bad utf-8"))?;
                    out.push_str(s);
                    self.i = start + width;
                }
            }
        }
    }

    fn parse_number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.bump();
        }
        match self.peek() {
            Some(b'0') => {
                self.bump();
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.bump();
                }
            }
            _ => return Err(self.err("invalid number")),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.bump();
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.i]).unwrap();
        if !is_float {
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Value::Int(i));
            }
        }
        let f: f64 = text.parse().map_err(|_| self.err("invalid number"))?;
        if !f.is_finite() {
            return Err(self.err("non-finite number"));
        }
        Ok(Value::Float(f))
    }
}

fn utf8_width(b: u8) -> Option<usize> {
    if b < 0x80 {
        Some(1)
    } else if b & 0b1110_0000 == 0b1100_0000 {
        Some(2)
    } else if b & 0b1111_0000 == 0b1110_0000 {
        Some(3)
    } else if b & 0b1111_1000 == 0b1111_0000 {
        Some(4)
    } else {
        None
    }
}

pub fn to_string(value: &Value) -> Result<String, Error> {
    let mut out = String::new();
    write_value(&mut out, value, 0, false)?;
    Ok(out)
}

pub fn to_string_pretty(value: &Value) -> Result<String, Error> {
    let mut out = String::new();
    write_value(&mut out, value, 0, true)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value, indent: usize, pretty: bool) -> Result<(), Error> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(Error { message: "refusing to emit NaN or Infinity".into() });
            }
            out.push_str(&format_float(*f));
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            if items.is_empty() {
                out.push(']');
                return Ok(());
            }
            for (i, item) in items.iter().enumerate() {
                if pretty {
                    out.push('\n');
                    push_indent(out, indent + 1);
                }
                write_value(out, item, indent + 1, pretty)?;
                if i + 1 != items.len() {
                    out.push(',');
                }
            }
            if pretty {
                out.push('\n');
                push_indent(out, indent);
            }
            out.push(']');
        }
        Value::Object(pairs) => {
            out.push('{');
            if pairs.is_empty() {
                out.push('}');
                return Ok(());
            }
            for (i, (k, v)) in pairs.iter().enumerate() {
                if pretty {
                    out.push('\n');
                    push_indent(out, indent + 1);
                }
                write_string(out, k);
                out.push(':');
                if pretty {
                    out.push(' ');
                }
                write_value(out, v, indent + 1, pretty)?;
                if i + 1 != pairs.len() {
                    out.push(',');
                }
            }
            if pretty {
                out.push('\n');
                push_indent(out, indent);
            }
            out.push('}');
        }
    }
    Ok(())
}

fn push_indent(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("  ");
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn format_float(f: f64) -> String {
    if !f.is_finite() || f == 0.0 {
        return "0.0".into();
    }
    let mut s = format!("{f}");
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        s.push_str(".0");
    }
    s
}

/// Python 3 `round` (half to even) at `digits` decimal places.
pub fn py_round(x: f64, digits: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if digits == 0 {
        return x.round_ties_even();
    }
    let p = 10f64.powi(digits);
    let scaled = x * p;
    // Nudge values that are a hair under/over an exact tie because of binary
    // scaling, matching the cases Python's decimal round hits for our scores.
    let nearest = scaled.round_ties_even();
    if (scaled - nearest).abs() < 1e-10 {
        nearest / p
    } else {
        scaled.round_ties_even() / p
    }
}

pub fn quantize_gap(value: f64) -> f64 {
    (py_round(value * 20.0, 0) / 20.0).min(1.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_duplicate_keys() {
        let v = parse(r#"{"a":1,"b":[true,null,"x"],"a":2}"#).unwrap();
        assert_eq!(v.get("a").and_then(|x| x.as_i64()), Some(2));
        let s = to_string(&v).unwrap();
        let again = parse(&s).unwrap();
        assert_eq!(v, again);
    }

    #[test]
    fn rejects_non_finite_and_deep() {
        assert!(parse("NaN").is_err());
        assert!(parse("Infinity").is_err());
        assert!(to_string(&Value::Float(f64::NAN)).is_err());
        let deep = "[".repeat(70) + &"]".repeat(70);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn bankers_rounding_matches_python_samples() {
        assert_eq!(py_round(0.5, 0), 0.0);
        assert_eq!(py_round(1.5, 0), 2.0);
        assert_eq!(py_round(2.5, 0), 2.0);
        assert_eq!(py_round(3.5, 0), 4.0);
        assert_eq!(py_round(0.25, 1), 0.2);
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(1.0 / 3.0, 4), 0.3333);
        assert_eq!(quantize_gap(0.025), 0.0);
        assert_eq!(quantize_gap(0.075), 0.1);
        assert_eq!(quantize_gap(0.05), 0.05);
        assert!((quantize_gap(1.0 / 3.0) - 0.35).abs() < 1e-12);
    }
}
