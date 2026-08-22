//! Lossless re-encoding of JSON, removing repetition but never a value.
//!
//! `gh <cmd> --json …` and `gh api …` are the outputs where most bytes are field
//! names repeated on every row, and both were full passthrough — tracked at 0%
//! savings. The objection recorded against compacting them was that converting
//! JSON to a schema destroys the values and forces a re-fetch. That objection is
//! to a *lossy* conversion; this module is lossless:
//!
//! - A top-level array of objects becomes a declaration line plus CSV rows.
//!   `[N]` declares the row count so truncation is detectable, `?` marks a
//!   nullable column, an empty cell means the key was absent, a bare `null`
//!   means JSON null, and a lookalike string (`"null"`, `"42"`) stays quoted.
//!   Mixed-type columns carry JSON literals, so `42` and `"42"` cannot blur.
//! - An envelope object stays *valid JSON*: dense inner arrays of objects become
//!   `{"_cols":[…],"_rows":[[…],…]}` tables, and the body is minified.
//!
//! # Losslessness is verified, not asserted
//!
//! [`pack`] decodes its own output and requires exact equality with the parsed
//! input **before emitting a single byte**. An encoder bug, or adversarial data
//! that collides with the notation itself (a real `{"_cols":…,"_rows":…}` object
//! in the payload), returns the raw bytes unchanged. The worst case of this
//! module is therefore the previous behaviour.
//!
//! Attribution: the lossless compaction idea is taken from the headroom-core
//! project (Apache-2.0); every lossy path it carries — retrieval pointers, row
//! dropping under budget, stringified-JSON rewriting — is deliberately absent.

use std::collections::HashMap;

use serde_json::{Map, Value};

/// Widening the flatten pass is quadratic in column count, so a pathological
/// object cannot be allowed to generate an unbounded header.
const MAX_COLUMNS: usize = 512;

/// Marker keys of the envelope table form. Payload data that already uses them
/// makes the notation ambiguous, and packing bails.
const COLS_KEY: &str = "_cols";
const ROWS_KEY: &str = "_rows";
const FLAT_KEY: &str = "_flat";

/// Minimum rows before an inner array is worth tabulating. Below this the header
/// costs more than the repetition it removes.
const MIN_TABLE_ROWS: usize = 2;

/// Losslessly repack `input`, or return `None` to keep the raw bytes.
///
/// `None` is returned for anything not worth packing, anything the encoder
/// cannot represent, and — critically — anything whose packed form does not
/// decode back to exactly the input.
pub fn pack(input: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(input).ok()?;

    let packed = match &parsed {
        Value::Array(rows) => pack_top_level_array(rows)?,
        Value::Object(_) => pack_envelope(&parsed)?,
        _ => return None,
    };

    // Verify before emitting: decode our own output and require exact equality.
    let decoded = unpack(&packed).ok()?;
    if decoded != parsed {
        return None;
    }
    if packed.len() >= input.len() {
        return None;
    }
    Some(packed)
}

/// Decode a packed payload back to the value it was built from.
///
/// This is the verifier [`pack`] runs against its own output; it is also what
/// makes the format checkable by a test rather than by inspection.
pub fn unpack(packed: &str) -> Result<Value, String> {
    if let Some(rest) = packed.strip_prefix('[') {
        return unpack_table(rest);
    }
    let value: Value = serde_json::from_str(packed).map_err(|error| error.to_string())?;
    Ok(detabulate(&value))
}

// ---------------------------------------------------------------------------
// Top-level array → declaration line + CSV rows
// ---------------------------------------------------------------------------

fn pack_top_level_array(rows: &[Value]) -> Option<String> {
    if rows.len() < MIN_TABLE_ROWS {
        return None;
    }
    let objects: Vec<&Map<String, Value>> = rows
        .iter()
        .map(|row| row.as_object())
        .collect::<Option<Vec<_>>>()?;

    let columns = collect_columns(&objects)?;
    if columns.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!("[{}]{{", rows.len()));
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&column.name);
        out.push(':');
        out.push_str(column.kind.as_str());
        if column.nullable {
            out.push('?');
        }
    }
    out.push_str("}\n");

    for object in &objects {
        let mut first = true;
        for column in &columns {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&encode_cell(object, column));
        }
        out.push('\n');
    }
    Some(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnKind {
    Int,
    Float,
    Bool,
    String,
    /// Values of more than one JSON type, or a nested container: every cell
    /// carries a JSON literal so the type can never be guessed wrong.
    Json,
}

impl ColumnKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Json => "json",
        }
    }

    fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            "int" => Self::Int,
            "float" => Self::Float,
            "bool" => Self::Bool,
            "string" => Self::String,
            "json" => Self::Json,
            _ => return None,
        })
    }

    fn of(value: &Value) -> Self {
        match value {
            Value::Bool(_) => Self::Bool,
            Value::Number(number) if number.is_i64() || number.is_u64() => Self::Int,
            Value::Number(_) => Self::Float,
            Value::String(_) => Self::String,
            _ => Self::Json,
        }
    }

    fn merge(self, other: Self) -> Self {
        if self == other {
            self
        } else if matches!(
            (&self, &other),
            (Self::Int, Self::Float) | (Self::Float, Self::Int)
        ) {
            Self::Float
        } else {
            Self::Json
        }
    }
}

#[derive(Debug, Clone)]
struct Column {
    /// Dotted path, e.g. `commit.author.name`.
    name: String,
    path: Vec<String>,
    kind: ColumnKind,
    nullable: bool,
    /// Rows that carried this key. Fewer than the row count means the key is
    /// absent somewhere, which is what makes the column nullable.
    present_rows: usize,
}

/// Build the column set, flattening uniform nested objects into dotted paths.
///
/// One pass over the rows, with a name→index map rather than a linear scan per
/// leaf: a `gh api` page is easily hundreds of rows wide by tens of columns, and
/// scanning the column vector for every leaf of every row made this quadratic in
/// the payload. Nullability is decided in the same pass by counting the rows
/// that set each column, instead of a second pass that re-walked every path.
fn collect_columns(objects: &[&Map<String, Value>]) -> Option<Vec<Column>> {
    let mut columns: Vec<Column> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut prefix: Vec<&str> = Vec::new();
    for object in objects {
        collect_from(object, &mut prefix, &mut columns, &mut index)?;
        debug_assert!(prefix.is_empty(), "prefix must unwind to empty per row");
    }
    // A key absent from some row is nullable, and so is an explicit JSON null
    // (recorded during the walk).
    for column in &mut columns {
        column.nullable |= column.present_rows < objects.len();
    }
    Some(columns)
}

fn collect_from<'a>(
    object: &'a Map<String, Value>,
    prefix: &mut Vec<&'a str>,
    columns: &mut Vec<Column>,
    index: &mut HashMap<String, usize>,
) -> Option<()> {
    for (key, value) in object {
        // A key carrying the separator or the marker prefix would make the
        // dotted path ambiguous on the way back.
        if key.contains('.') || key.contains(',') || key.starts_with('_') {
            return None;
        }
        prefix.push(key.as_str());
        match value {
            Value::Object(inner) if !inner.is_empty() => {
                collect_from(inner, prefix, columns, index)?;
            }
            _ => {
                let name = prefix.join(".");
                let kind = ColumnKind::of(value);
                match index.get(&name) {
                    Some(&position) => {
                        let existing = &mut columns[position];
                        existing.kind = existing.kind.merge(kind);
                        existing.nullable |= value.is_null();
                        existing.present_rows += 1;
                    }
                    None => {
                        index.insert(name.clone(), columns.len());
                        columns.push(Column {
                            name,
                            path: prefix
                                .iter()
                                .map(|segment| (*segment).to_string())
                                .collect(),
                            kind,
                            nullable: value.is_null(),
                            present_rows: 1,
                        });
                        if columns.len() > MAX_COLUMNS {
                            return None;
                        }
                    }
                }
            }
        }
        prefix.pop();
    }
    Some(())
}

fn lookup<'a>(object: &'a Map<String, Value>, path: &[String]) -> Option<&'a Value> {
    let mut current = object;
    for (index, segment) in path.iter().enumerate() {
        let value = current.get(segment)?;
        if index + 1 == path.len() {
            return Some(value);
        }
        current = value.as_object()?;
    }
    None
}

fn encode_cell(object: &Map<String, Value>, column: &Column) -> String {
    let Some(value) = lookup(object, &column.path) else {
        // Absent key: an empty cell, distinct from an explicit null.
        return String::new();
    };
    match (value, &column.kind) {
        (Value::Null, _) => "null".to_string(),
        (Value::String(text), ColumnKind::String) => csv_quote(text),
        (_, ColumnKind::Json) => csv_quote(&value.to_string()),
        _ => csv_quote(&value.to_string()),
    }
}

/// Quote a cell when the raw text would be ambiguous: it contains a separator,
/// a quote, a newline, leading/trailing space, or it looks like the `null`
/// sentinel or an empty cell.
fn csv_quote(text: &str) -> String {
    let needs_quote = text.is_empty()
        || text == "null"
        || text.contains(',')
        || text.contains('"')
        || text.contains('\n')
        || text.contains('\r')
        || text.starts_with(' ')
        || text.ends_with(' ');
    if !needs_quote {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
    out
}

fn unpack_table(rest: &str) -> Result<Value, String> {
    let (count_text, after_count) = rest
        .split_once(']')
        .ok_or_else(|| "missing row-count terminator".to_string())?;
    let declared: usize = count_text
        .parse()
        .map_err(|_| "row count is not a number".to_string())?;

    let after_count = after_count
        .strip_prefix('{')
        .ok_or_else(|| "missing column declaration".to_string())?;
    let (header, body) = after_count
        .split_once("}\n")
        .ok_or_else(|| "unterminated column declaration".to_string())?;

    let mut columns = Vec::new();
    for spec in header.split(',') {
        let (name, kind_text) = spec
            .rsplit_once(':')
            .ok_or_else(|| format!("column '{spec}' has no type"))?;
        let (kind_text, nullable) = match kind_text.strip_suffix('?') {
            Some(stripped) => (stripped, true),
            None => (kind_text, false),
        };
        let kind = ColumnKind::from_str(kind_text)
            .ok_or_else(|| format!("unknown column type '{kind_text}'"))?;
        columns.push(Column {
            name: name.to_string(),
            path: name.split('.').map(str::to_string).collect(),
            kind,
            nullable,
            // Decoding reads the declaration; the count only matters while
            // building one from rows.
            present_rows: 0,
        });
    }

    let mut rows = Vec::with_capacity(declared);
    for line in body.lines() {
        let cells = split_csv_row(line, columns.len())?;
        let mut object = Map::new();
        for (column, cell) in columns.iter().zip(cells) {
            let Some(value) = decode_cell(&cell, column)? else {
                continue; // absent key
            };
            insert_path(&mut object, &column.path, value);
        }
        rows.push(Value::Object(object));
    }
    if rows.len() != declared {
        return Err(format!("declared {declared} rows, decoded {}", rows.len()));
    }
    Ok(Value::Array(rows))
}

#[derive(PartialEq)]
enum Cell {
    Absent,
    Quoted(String),
    Bare(String),
}

fn split_csv_row(line: &str, expected: usize) -> Result<Vec<Cell>, String> {
    let mut cells = Vec::with_capacity(expected);
    let mut chars = line.chars().peekable();
    loop {
        let cell = if chars.peek() == Some(&'"') {
            chars.next();
            let mut text = String::new();
            loop {
                match chars.next() {
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        text.push('"');
                    }
                    Some('"') => break,
                    Some(character) => text.push(character),
                    None => return Err("unterminated quoted cell".to_string()),
                }
            }
            Cell::Quoted(text)
        } else {
            let mut text = String::new();
            while let Some(&character) = chars.peek() {
                if character == ',' {
                    break;
                }
                text.push(character);
                chars.next();
            }
            if text.is_empty() {
                Cell::Absent
            } else {
                Cell::Bare(text)
            }
        };
        cells.push(cell);
        match chars.next() {
            Some(',') => continue,
            Some(other) => return Err(format!("unexpected character '{other}' after cell")),
            None => break,
        }
    }
    if cells.len() != expected {
        return Err(format!("expected {expected} cells, found {}", cells.len()));
    }
    Ok(cells)
}

fn decode_cell(cell: &Cell, column: &Column) -> Result<Option<Value>, String> {
    let text = match cell {
        Cell::Absent => return Ok(None),
        Cell::Bare(text) if text == "null" => return Ok(Some(Value::Null)),
        Cell::Bare(text) => text,
        Cell::Quoted(text) => {
            return Ok(Some(match column.kind {
                ColumnKind::String => Value::String(text.clone()),
                ColumnKind::Json => {
                    serde_json::from_str(text).map_err(|error| error.to_string())?
                }
                _ => serde_json::from_str(text).map_err(|error| error.to_string())?,
            }));
        }
    };
    Ok(Some(match column.kind {
        ColumnKind::String => Value::String(text.clone()),
        _ => serde_json::from_str(text).map_err(|error| error.to_string())?,
    }))
}

fn insert_path(object: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = object;
    for segment in parents {
        current = current
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("path segment is an object by construction");
    }
    current.insert(last.clone(), value);
}

// ---------------------------------------------------------------------------
// Envelope object → minified JSON with tabulated inner arrays
// ---------------------------------------------------------------------------

fn pack_envelope(value: &Value) -> Option<String> {
    if contains_marker_keys(value) {
        // Payload data already uses the table markers; the notation would be
        // ambiguous on the way back.
        return None;
    }
    let tabulated = tabulate(value)?;
    if tabulated == *value {
        // Nothing was tabulated; minification alone is handled by the size check
        // in `pack`, which rejects a result that is not smaller.
        return serde_json::to_string(value).ok();
    }
    serde_json::to_string(&tabulated).ok()
}

fn contains_marker_keys(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(COLS_KEY)
                || map.contains_key(ROWS_KEY)
                || map.contains_key(FLAT_KEY)
                || map.values().any(contains_marker_keys)
        }
        Value::Array(items) => items.iter().any(contains_marker_keys),
        _ => false,
    }
}

fn tabulate(value: &Value) -> Option<Value> {
    Some(match value {
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (key, inner) in map {
                out.insert(key.clone(), tabulate(inner)?);
            }
            Value::Object(out)
        }
        Value::Array(items) => match table_form(items) {
            Some(table) => table,
            None => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(tabulate(item)?);
                }
                Value::Array(out)
            }
        },
        other => other.clone(),
    })
}

/// `{"_cols":[…],"_rows":[[…]]}` for a dense array of flat objects, else `None`.
fn table_form(items: &[Value]) -> Option<Value> {
    if items.len() < MIN_TABLE_ROWS {
        return None;
    }
    let objects: Vec<&Map<String, Value>> = items
        .iter()
        .map(|item| item.as_object())
        .collect::<Option<Vec<_>>>()?;
    // Only flat objects tabulate here; nesting keeps its own shape so the
    // round-trip stays exact without a flatten marker.
    if objects
        .iter()
        .any(|object| object.values().any(|value| value.is_object()))
    {
        return None;
    }

    let mut columns: Vec<String> = Vec::new();
    for object in &objects {
        for key in object.keys() {
            if key.starts_with('_') {
                return None;
            }
            if !columns.iter().any(|existing| existing == key) {
                columns.push(key.clone());
                if columns.len() > MAX_COLUMNS {
                    return None;
                }
            }
        }
    }
    // Sparse rows would need an absent marker inside the row arrays; leave those
    // alone rather than inventing one.
    if objects.iter().any(|object| object.len() != columns.len()) {
        return None;
    }

    let rows: Vec<Value> = objects
        .iter()
        .map(|object| {
            Value::Array(
                columns
                    .iter()
                    .map(|column| object[column].clone())
                    .collect(),
            )
        })
        .collect();

    let mut table = Map::new();
    table.insert(
        COLS_KEY.to_string(),
        Value::Array(columns.into_iter().map(Value::String).collect()),
    );
    table.insert(ROWS_KEY.to_string(), Value::Array(rows));
    Some(Value::Object(table))
}

/// Rebuild the original value from a tabulated envelope. This is the inverse of
/// [`tabulate`], and the reason [`pack`] can verify its own output.
fn detabulate(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if let (Some(Value::Array(cols)), Some(Value::Array(rows))) =
                (map.get(COLS_KEY), map.get(ROWS_KEY))
            {
                if map.len() == 2 {
                    let names: Vec<&str> = cols.iter().filter_map(|col| col.as_str()).collect();
                    if names.len() == cols.len() {
                        return Value::Array(
                            rows.iter()
                                .map(|row| {
                                    let mut object = Map::new();
                                    if let Value::Array(cells) = row {
                                        for (name, cell) in names.iter().zip(cells) {
                                            object.insert((*name).to_string(), detabulate(cell));
                                        }
                                    }
                                    Value::Object(object)
                                })
                                .collect(),
                        );
                    }
                }
            }
            Value::Object(
                map.iter()
                    .map(|(key, inner)| (key.clone(), detabulate(inner)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(detabulate).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(input: &str) {
        let parsed: Value = serde_json::from_str(input).expect("valid fixture");
        let Some(packed) = pack(input) else {
            return; // declining to pack is always allowed
        };
        let decoded = unpack(&packed)
            .unwrap_or_else(|error| panic!("packed form did not decode: {error}\n{packed}"));
        assert_eq!(decoded, parsed, "packed form lost data:\n{packed}");
        assert!(packed.len() < input.len(), "packed form was not smaller");
    }

    #[test]
    fn array_of_objects_packs_and_round_trips() {
        let input = r#"[
          {"id": 1, "author": {"login": "alice"}, "title": "First title", "tag": "x"},
          {"id": 2, "author": {"login": "bob"}, "title": "Title, with a comma"},
          {"id": 3, "author": {"login": "carol"}, "title": "Third title", "tag": null}
        ]"#;
        roundtrip(input);
        let packed = pack(input).expect("packs");
        assert!(packed.starts_with("[3]{"), "declaration line: {packed}");
        assert!(packed.contains("author.login:string"), "{packed}");
    }

    #[test]
    fn absent_null_empty_and_lookalikes_stay_distinct() {
        let input = r#"[
          {"a": "null", "b": "", "c": "42", "d": 42},
          {"a": null, "b": " padded ", "c": "x", "d": 7},
          {"b": "plain", "c": "y", "d": 9}
        ]"#;
        roundtrip(input);
    }

    #[test]
    fn mixed_type_columns_do_not_blur() {
        roundtrip(r#"[{"v": 42}, {"v": "42"}, {"v": true}]"#);
    }

    #[test]
    fn commas_quotes_and_newlines_survive() {
        roundtrip(r#"[{"t":"a,b"},{"t":"say \"hi\""},{"t":"line1\nline2"},{"t":"tail\r"}]"#);
    }

    #[test]
    fn extreme_numbers_keep_their_digits() {
        roundtrip(
            r#"[{"n": 1.10, "big": 9007199254740993, "neg": -0.000001},
                {"n": 2.5, "big": 1, "neg": 3.0},
                {"n": 0.1, "big": 2, "neg": 1e10}]"#,
        );
    }

    #[test]
    fn envelope_object_stays_valid_json() {
        let input = r#"{"total_count": 3, "items": [
            {"id": 1, "name": "a"},
            {"id": 2, "name": "b"},
            {"id": 3, "name": "c"}
        ]}"#;
        let packed = pack(input).expect("packs");
        serde_json::from_str::<Value>(&packed).expect("envelope stays valid JSON");
        assert!(packed.contains(COLS_KEY), "inner array tabulated: {packed}");
        assert_eq!(
            unpack(&packed).unwrap(),
            serde_json::from_str::<Value>(input).unwrap()
        );
    }

    #[test]
    fn payload_using_the_table_markers_is_refused() {
        // Adversarial: real data that already looks like the notation.
        let input = r#"{"x": {"_cols": ["a"], "_rows": [[1]]}}"#;
        assert_eq!(pack(input), None);
    }

    #[test]
    fn dotted_and_underscore_keys_are_refused() {
        assert_eq!(pack(r#"[{"a.b": 1}, {"a.b": 2}]"#), None);
        assert_eq!(pack(r#"[{"_x": 1}, {"_x": 2}]"#), None);
    }

    #[test]
    fn tiny_and_non_tabular_payloads_are_left_alone() {
        assert_eq!(pack("[]"), None);
        assert_eq!(pack(r#"[{"a":1}]"#), None);
        assert_eq!(pack(r#"[1,2,3]"#), None);
        assert_eq!(pack("not json"), None);
        assert_eq!(pack("\"scalar\""), None);
    }

    #[test]
    fn a_packed_form_that_is_not_smaller_is_refused() {
        // Long key names with one row each way: the declaration line costs more
        // than the repetition it removes, so the raw bytes win.
        let input = r#"[{"x":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"x":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]"#;
        let packed = pack(input);
        if let Some(packed) = &packed {
            assert!(
                packed.len() < input.len(),
                "pack must never return a larger form"
            );
        }
        // And the guard itself holds for every fixture in this module.
        for fixture in [r#"[{"a":1},{"a":2}]"#, r#"[{"k":"v"},{"k":"w"},{"k":"x"}]"#] {
            if let Some(packed) = pack(fixture) {
                assert!(packed.len() < fixture.len(), "{fixture} -> {packed}");
            }
        }
    }

    #[test]
    fn sparse_inner_arrays_are_not_tabulated() {
        let input = r#"{"items":[{"a":1,"b":2},{"a":3}]}"#;
        // Either refused, or packed and exactly recoverable — never lossy.
        if let Some(packed) = pack(input) {
            let value: Value = serde_json::from_str(&packed).unwrap();
            assert_eq!(
                detabulate(&value),
                serde_json::from_str::<Value>(input).unwrap()
            );
        }
    }

    #[test]
    fn wide_and_tall_payloads_stay_linear_and_lossless() {
        // 400 columns x 300 rows. Under the previous linear column scan this
        // was ~36M string comparisons per pack; the name index makes it one
        // hash lookup per leaf. The assertion here is correctness — the cost is
        // what makes the test finish.
        let mut rows = Vec::with_capacity(300);
        for row in 0..300 {
            let mut object = Map::new();
            for column in 0..400 {
                object.insert(format!("c{column}"), Value::from(row * 400 + column));
            }
            rows.push(Value::Object(object));
        }
        let input = Value::Array(rows).to_string();
        let packed = pack(&input).expect("a wide, tall table packs");
        assert_eq!(
            unpack(&packed).expect("round-trips"),
            serde_json::from_str::<Value>(&input).unwrap()
        );
    }

    #[test]
    fn a_key_absent_from_a_later_row_is_still_nullable() {
        // Nullability used to come from a second pass over every path; it is now
        // decided from a per-column presence count in the same walk, so a key
        // that disappears only in the last row must still mark the column.
        let input = r#"[{"a":1,"b":2},{"a":3,"b":4},{"a":5}]"#;
        let packed = pack(input).expect("packs");
        assert!(
            packed.contains("b:int?"),
            "column b must be marked nullable: {packed}"
        );
        assert_eq!(
            unpack(&packed).unwrap(),
            serde_json::from_str::<Value>(input).unwrap()
        );
    }

    #[test]
    fn column_cap_bails_instead_of_exploding() {
        let mut wide = String::from("[");
        for row in 0..3 {
            if row > 0 {
                wide.push(',');
            }
            wide.push('{');
            for column in 0..(MAX_COLUMNS + 10) {
                if column > 0 {
                    wide.push(',');
                }
                wide.push_str(&format!("\"c{column}\":{row}"));
            }
            wide.push('}');
        }
        wide.push(']');
        assert_eq!(pack(&wide), None);
    }

    #[test]
    fn realistic_issue_list_round_trips_and_saves() {
        let input = r#"[
          {"number": 3631, "title": "fix(git): announce a failed commit", "state": "OPEN", "author": {"login": "poelzi"}},
          {"number": 3630, "title": "fix(copilot): make hook work", "state": "OPEN", "author": {"login": "rNoz"}},
          {"number": 3629, "title": "feat(filters): add sqlite3 TOML filter", "state": "OPEN", "author": {"login": "saddestmartian"}},
          {"number": 3627, "title": "docs(gh): clarify api passthrough", "state": "MERGED", "author": {"login": "nyxst4ck"}}
        ]"#;
        roundtrip(input);
        let packed = pack(input).expect("packs");
        // Compare against the minified form, not the pretty fixture, so the
        // measurement is of repetition removed rather than whitespace removed.
        let minified =
            serde_json::to_string(&serde_json::from_str::<Value>(input).unwrap()).unwrap();
        assert!(
            packed.len() * 4 < minified.len() * 3,
            "expected >25% off the minified form, got {} from {}",
            packed.len(),
            minified.len()
        );
    }
}
