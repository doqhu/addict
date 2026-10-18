//! A read-only reader of the rows of one table of an SQLite database file,
//! held in memory: what the WikDict files need, and nothing more.
//!
//! The subset it reads, from the SQLite file format: the 100-byte header
//! (rollback journal mode, UTF-8 text), the `sqlite_schema` table on page 1
//! to find the root page and the columns of a table, table b-trees (interior
//! and leaf pages), records, and the overflow pages of a payload that does
//! not fit its page. A table is read in rowid order. Anything outside that
//! subset is refused with an [`Error`]: another text encoding, a WAL
//! database, a table `WITHOUT ROWID` or one with a `PRIMARY KEY` (whose
//! integer key would be the rowid, not in the record), an index page where a
//! table page should be. Lengths and page numbers are never trusted: what
//! does not fit the file is an error, and so is a page reached twice.
//!
//! There is no SQL engine: the columns of a table are named in its `CREATE
//! TABLE` statement, read here as a comma-separated list.

use std::fmt;

/// The first bytes of every SQLite database file.
const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// The length of the database header at the start of page 1.
const HEADER_LEN: usize = 100;

/// The deepest b-tree the reader follows.
const MAX_DEPTH: usize = 64;

/// One value of a record.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// Why a database could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn error<T>(what: impl Into<String>) -> Result<T, Error> {
    Err(Error(what.into()))
}

/// An SQLite database file in memory.
pub struct Database<'a> {
    bytes: &'a [u8],
    page_size: usize,
    /// The bytes of a page that hold its contents: the page size less the
    /// reserved bytes at its end.
    usable: usize,
    pages: u32,
}

impl<'a> Database<'a> {
    /// Reads the header of the database `bytes`.
    pub fn open(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_LEN || &bytes[..16] != MAGIC {
            return error("not an SQLite database");
        }
        let page_size = match u16::from_be_bytes([bytes[16], bytes[17]]) {
            1 => 65536,
            size => usize::from(size),
        };
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
            return error(format!("the page size {page_size} is not valid"));
        }
        if bytes[18] != 1 || bytes[19] != 1 {
            return error("the database is not in rollback journal mode");
        }
        let usable = page_size - usize::from(bytes[20]);
        if usable < 480 {
            return error("too many reserved bytes per page");
        }
        if bytes[21..24] != [64, 32, 32] {
            return error("the payload fractions are not 64, 32 and 32");
        }
        if u32::from_be_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]) != 1 {
            return error("the text encoding is not UTF-8");
        }
        if !bytes.len().is_multiple_of(page_size) {
            return error("the file is not a whole number of pages");
        }
        let pages = u32::try_from(bytes.len() / page_size)
            .or_else(|_| error("the file has too many pages"))?;
        Ok(Self {
            bytes,
            page_size,
            usable,
            pages,
        })
    }

    /// The bytes of page `number`, counted from 1.
    fn page(&self, number: u32) -> Result<&'a [u8], Error> {
        if number == 0 || number > self.pages {
            return error(format!("a page number {number} is outside the file"));
        }
        let start = (number as usize - 1) * self.page_size;
        Ok(&self.bytes[start..start + self.usable])
    }

    /// The values of `columns` of every row of the table `name`, in rowid
    /// order. A column a row does not have, because it was added later, is
    /// [`Value::Null`].
    pub fn rows(&self, name: &str, columns: &[&str]) -> Result<Vec<Vec<Value>>, Error> {
        let (root, sql) = self.table(name)?;
        let names = column_names(&sql).ok_or_else(|| {
            Error(format!(
                "the table {name} is not one this reader can read: {}",
                one_line(&sql)
            ))
        })?;
        let at: Vec<usize> = columns
            .iter()
            .map(|column| {
                names
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(column))
                    .ok_or_else(|| Error(format!("the table {name} has no column {column}")))
            })
            .collect::<Result<_, _>>()?;
        let mut rows = Vec::new();
        self.walk(root, &mut |mut record| {
            rows.push(
                at.iter()
                    .map(|at| match record.get_mut(*at) {
                        Some(value) => std::mem::replace(value, Value::Null),
                        None => Value::Null,
                    })
                    .collect(),
            );
            Ok(())
        })?;
        Ok(rows)
    }

    /// The root page and the `CREATE TABLE` statement of the table `name`.
    fn table(&self, name: &str) -> Result<(u32, String), Error> {
        let mut found = None;
        self.walk(1, &mut |record| {
            if let [
                Value::Text(kind),
                Value::Text(own),
                _,
                root,
                Value::Text(sql),
                ..,
            ] = record.as_slice()
                && kind == "table"
                && own.eq_ignore_ascii_case(name)
                && found.is_none()
            {
                let Value::Integer(root) = root else {
                    return error(format!("the table {name} has no root page"));
                };
                let root = u32::try_from(*root)
                    .or_else(|_| error(format!("the table {name} has no root page")))?;
                found = Some((root, sql.clone()));
            }
            Ok(())
        })?;
        found.ok_or_else(|| Error(format!("the database has no table {name}")))
    }

    /// Calls `row` with the record of every cell of the table b-tree whose
    /// root is page `root`, in key order.
    fn walk(
        &self,
        root: u32,
        row: &mut dyn FnMut(Vec<Value>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut seen = vec![false; self.pages as usize + 1];
        self.walk_page(root, 0, &mut seen, row)
    }

    fn walk_page(
        &self,
        number: u32,
        depth: usize,
        seen: &mut [bool],
        row: &mut dyn FnMut(Vec<Value>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return error("a b-tree is too deep");
        }
        let page = self.page(number)?;
        visit(seen, number)?;
        let header = if number == 1 { HEADER_LEN } else { 0 };
        let kind = *page
            .get(header)
            .ok_or_else(|| Error(format!("page {number} is too short")))?;
        let interior = match kind {
            5 => true,
            13 => false,
            _ => {
                return error(format!(
                    "page {number} is not a table b-tree page (type {kind})"
                ));
            }
        };
        let header_len = if interior { 12 } else { 8 };
        let fixed = page
            .get(header..header + header_len)
            .ok_or_else(|| Error(format!("page {number} is too short")))?;
        let cells = usize::from(u16::from_be_bytes([fixed[3], fixed[4]]));
        let pointers = page
            .get(header + header_len..header + header_len + 2 * cells)
            .ok_or_else(|| Error(format!("the cells of page {number} do not fit it")))?;
        for pointer in pointers.as_chunks::<2>().0 {
            let offset = usize::from(u16::from_be_bytes([pointer[0], pointer[1]]));
            let cell = page
                .get(offset..)
                .filter(|cell| !cell.is_empty() && offset >= header + header_len + 2 * cells)
                .ok_or_else(|| Error(format!("a cell of page {number} is outside it")))?;
            if interior {
                let child = cell
                    .get(..4)
                    .ok_or_else(|| Error(format!("a cell of page {number} is cut")))?;
                let child = u32::from_be_bytes([child[0], child[1], child[2], child[3]]);
                // The key is the largest rowid of the child: not needed.
                varint(&cell[4..])
                    .ok_or_else(|| Error(format!("a cell of page {number} is cut")))?;
                self.walk_page(child, depth + 1, seen, row)?;
            } else {
                row(record(&self.payload(number, cell, seen)?)?)?;
            }
        }
        if interior {
            let right = u32::from_be_bytes([fixed[8], fixed[9], fixed[10], fixed[11]]);
            self.walk_page(right, depth + 1, seen, row)?;
        }
        Ok(())
    }

    /// The payload of the leaf cell `cell` of page `number`, its overflow
    /// pages followed.
    fn payload(&self, number: u32, cell: &[u8], seen: &mut [bool]) -> Result<Vec<u8>, Error> {
        let cut = || Error(format!("a cell of page {number} is cut"));
        let (size, read) = varint(cell).ok_or_else(cut)?;
        let (_rowid, rowid_len) = varint(&cell[read..]).ok_or_else(cut)?;
        let body = &cell[read + rowid_len..];
        let size = usize::try_from(size)
            .ok()
            .filter(|size| *size <= self.bytes.len())
            .ok_or_else(|| {
                Error(format!(
                    "a payload of page {number} is larger than the file"
                ))
            })?;
        let local = self.local(size);
        let mut payload = body.get(..local).ok_or_else(cut)?.to_vec();
        if local == size {
            return Ok(payload);
        }
        let next = body.get(local..local + 4).ok_or_else(cut)?;
        let mut next = u32::from_be_bytes([next[0], next[1], next[2], next[3]]);
        while payload.len() < size {
            let page = self.page(next)?;
            visit(seen, next)?;
            next = u32::from_be_bytes([page[0], page[1], page[2], page[3]]);
            let take = (size - payload.len()).min(self.usable - 4);
            payload.extend_from_slice(&page[4..4 + take]);
        }
        Ok(payload)
    }

    /// How much of a payload of `size` bytes is on its leaf page: the rule
    /// of the file format for table leaf cells.
    fn local(&self, size: usize) -> usize {
        let usable = self.usable;
        let most = usable - 35;
        if size <= most {
            return size;
        }
        let least = (usable - 12) * 32 / 255 - 23;
        let local = least + (size - least) % (usable - 4);
        if local <= most { local } else { least }
    }
}

/// Marks page `number` as read; a page read twice is a loop of the file.
fn visit(seen: &mut [bool], number: u32) -> Result<(), Error> {
    let seen = &mut seen[number as usize];
    if *seen {
        return error(format!("page {number} is reached twice"));
    }
    *seen = true;
    Ok(())
}

/// A varint of the file format and its length in bytes; `None` when cut.
fn varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for (at, byte) in bytes.iter().enumerate().take(9) {
        if at == 8 {
            return Some(((value << 8) | u64::from(*byte), 9));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((value, at + 1));
        }
    }
    None
}

/// The values of the record `payload`.
fn record(payload: &[u8]) -> Result<Vec<Value>, Error> {
    let bad = || Error("a record is malformed".to_owned());
    let (header_len, mut at) = varint(payload).ok_or_else(bad)?;
    let header_len = usize::try_from(header_len)
        .ok()
        .filter(|len| *len >= at && *len <= payload.len())
        .ok_or_else(bad)?;
    let mut body = header_len;
    let mut values = Vec::new();
    while at < header_len {
        let (serial, read) = varint(&payload[at..header_len]).ok_or_else(bad)?;
        at += read;
        let len = match serial {
            0 | 8 | 9 => 0,
            1..=4 => serial as usize,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return error("a record has a reserved serial type"),
            _ => usize::try_from((serial - 12) / 2).map_err(|_| bad())?,
        };
        let bytes = payload
            .get(body..body.checked_add(len).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        body += len;
        values.push(match serial {
            0 => Value::Null,
            1..=6 => {
                // Big-endian two's complement, sign-extended.
                let mut value = if bytes[0] & 0x80 != 0 { -1i64 } else { 0 };
                for byte in bytes {
                    value = (value << 8) | i64::from(*byte);
                }
                Value::Integer(value)
            }
            7 => Value::Real(f64::from_be_bytes(bytes.try_into().map_err(|_| bad())?)),
            8 => Value::Integer(0),
            9 => Value::Integer(1),
            _ if serial % 2 == 0 => Value::Blob(bytes.to_vec()),
            _ => Value::Text(
                String::from_utf8(bytes.to_vec())
                    .or_else(|_| error("a text of a record is not UTF-8"))?,
            ),
        });
    }
    Ok(values)
}

/// The column names of a `CREATE TABLE name(column, ...)` statement, in
/// order; `None` for a statement outside the subset: no column list, a
/// `WITHOUT ROWID` table, or a `PRIMARY KEY`.
fn column_names(sql: &str) -> Option<Vec<String>> {
    let open = sql.find('(')?;
    let close = sql.rfind(')')?;
    let upper = sql.to_ascii_uppercase();
    if close < open || upper.contains("PRIMARY KEY") || upper[close..].contains("WITHOUT") {
        return None;
    }
    let mut names = Vec::new();
    for definition in split_top_level(&sql[open + 1..close]) {
        let definition = definition.trim();
        let first = definition
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()?
            .to_ascii_uppercase();
        if matches!(
            first.as_str(),
            "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN"
        ) {
            continue;
        }
        names.push(column_name(definition)?);
    }
    (!names.is_empty()).then_some(names)
}

/// The column definitions of a column list: split at the commas outside
/// parentheses and quotes.
fn split_top_level(list: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0usize, None, 0);
    for (at, c) in list.char_indices() {
        match (quote, c) {
            (Some(open), _) if c == close_quote(open) => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`' | '[') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                parts.push(&list[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&list[start..]);
    parts
}

fn close_quote(open: char) -> char {
    if open == '[' { ']' } else { open }
}

/// The name a column definition starts with, unquoted.
fn column_name(definition: &str) -> Option<String> {
    let mut characters = definition.chars();
    let first = characters.next()?;
    if matches!(first, '"' | '`' | '[') {
        let end = definition[1..].find(close_quote(first))?;
        return Some(definition[1..1 + end].to_owned());
    }
    let name: String = definition
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '(')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn put_varint(out: &mut Vec<u8>, value: u64) {
        if value > 0x00ff_ffff_ffff_ffff {
            let mut bytes = Vec::new();
            bytes.push(value as u8);
            let mut rest = value >> 8;
            for _ in 0..8 {
                bytes.push((rest & 0x7f) as u8 | 0x80);
                rest >>= 7;
            }
            bytes.reverse();
            out.extend_from_slice(&bytes);
            return;
        }
        let mut groups = vec![(value & 0x7f) as u8];
        let mut rest = value >> 7;
        while rest > 0 {
            groups.push((rest & 0x7f) as u8 | 0x80);
            rest >>= 7;
        }
        groups.reverse();
        out.extend_from_slice(&groups);
    }

    /// The record of `values`, as SQLite writes it.
    pub(crate) fn record_of(values: &[Value]) -> Vec<u8> {
        let mut types = Vec::new();
        let mut body = Vec::new();
        for value in values {
            let serial = match value {
                Value::Null => 0,
                Value::Integer(0) => 8,
                Value::Integer(1) => 9,
                Value::Integer(value) => {
                    body.extend_from_slice(&value.to_be_bytes());
                    6
                }
                Value::Real(value) => {
                    body.extend_from_slice(&value.to_be_bytes());
                    7
                }
                Value::Text(text) => {
                    body.extend_from_slice(text.as_bytes());
                    text.len() as u64 * 2 + 13
                }
                Value::Blob(bytes) => {
                    body.extend_from_slice(bytes);
                    bytes.len() as u64 * 2 + 12
                }
            };
            put_varint(&mut types, serial);
        }
        // The header length counts itself; one byte is enough here.
        let mut record = vec![(types.len() + 1) as u8];
        assert!(types.len() + 1 < 0x80);
        record.extend_from_slice(&types);
        record.extend_from_slice(&body);
        record
    }

    /// A database file built page by page, with small pages so that a few
    /// rows need interior pages and overflow pages.
    pub(crate) struct Builder {
        page_size: usize,
        /// Page 1 is index 0; it is written by [`Builder::finish`].
        pages: Vec<Vec<u8>>,
    }

    impl Builder {
        pub(crate) fn new(page_size: usize) -> Self {
            Self {
                page_size,
                pages: vec![Vec::new()],
            }
        }

        fn allocate(&mut self) -> u32 {
            self.pages.push(vec![0; self.page_size]);
            self.pages.len() as u32
        }

        /// The cell of a row of a table leaf, with its overflow pages
        /// written.
        fn leaf_cell(&mut self, rowid: u64, payload: &[u8]) -> Vec<u8> {
            let usable = self.page_size;
            let most = usable - 35;
            let mut cell = Vec::new();
            put_varint(&mut cell, payload.len() as u64);
            put_varint(&mut cell, rowid);
            if payload.len() <= most {
                cell.extend_from_slice(payload);
                return cell;
            }
            let least = (usable - 12) * 32 / 255 - 23;
            let mut local = least + (payload.len() - least) % (usable - 4);
            if local > most {
                local = least;
            }
            cell.extend_from_slice(&payload[..local]);
            let chunks: Vec<&[u8]> = payload[local..].chunks(usable - 4).collect();
            let numbers: Vec<u32> = chunks.iter().map(|_| self.allocate()).collect();
            cell.extend_from_slice(&numbers[0].to_be_bytes());
            for (at, chunk) in chunks.iter().enumerate() {
                let page = &mut self.pages[numbers[at] as usize - 1];
                let next = numbers.get(at + 1).copied().unwrap_or(0);
                page[..4].copy_from_slice(&next.to_be_bytes());
                page[4..4 + chunk.len()].copy_from_slice(chunk);
            }
            cell
        }

        /// Writes a b-tree page of `kind` with `cells` at `number`.
        fn write_page(&mut self, number: u32, kind: u8, cells: &[Vec<u8>], right: Option<u32>) {
            let header = if number == 1 { HEADER_LEN } else { 0 };
            let mut page = vec![0; self.page_size];
            page[header] = kind;
            page[header + 3..header + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
            let header_len = if let Some(right) = right {
                page[header + 8..header + 12].copy_from_slice(&right.to_be_bytes());
                12
            } else {
                8
            };
            let mut end = self.page_size;
            for (at, cell) in cells.iter().enumerate() {
                end -= cell.len();
                assert!(
                    end >= header + header_len + 2 * cells.len(),
                    "page {number} is full"
                );
                page[end..end + cell.len()].copy_from_slice(cell);
                let pointer = header + header_len + 2 * at;
                page[pointer..pointer + 2].copy_from_slice(&(end as u16).to_be_bytes());
            }
            page[header + 5..header + 7].copy_from_slice(&(end as u16).to_be_bytes());
            self.pages[number as usize - 1] = page;
        }

        /// A leaf page of rows, each a rowid and its values.
        pub(crate) fn leaf(&mut self, rows: &[(u64, Vec<Value>)]) -> u32 {
            let number = self.allocate();
            let cells: Vec<Vec<u8>> = rows
                .iter()
                .map(|(rowid, values)| self.leaf_cell(*rowid, &record_of(values)))
                .collect();
            self.write_page(number, 13, &cells, None);
            number
        }

        /// An interior page over `children`, each a page and its largest
        /// rowid, and the right-most child `right`.
        pub(crate) fn interior(&mut self, children: &[(u32, u64)], right: u32) -> u32 {
            let number = self.allocate();
            let cells: Vec<Vec<u8>> = children
                .iter()
                .map(|(child, key)| {
                    let mut cell = child.to_be_bytes().to_vec();
                    put_varint(&mut cell, *key);
                    cell
                })
                .collect();
            self.write_page(number, 5, &cells, Some(right));
            number
        }

        /// The bytes of the file, with `schema` rows (type, name, root,
        /// sql) on page 1.
        pub(crate) fn finish(mut self, schema: &[(&str, &str, u32, &str)]) -> Vec<u8> {
            let rows: Vec<Vec<u8>> = schema
                .iter()
                .enumerate()
                .map(|(at, (kind, name, root, sql))| {
                    let record = record_of(&[
                        Value::Text((*kind).to_owned()),
                        Value::Text((*name).to_owned()),
                        Value::Text((*name).to_owned()),
                        Value::Integer(i64::from(*root)),
                        Value::Text((*sql).to_owned()),
                    ]);
                    self.leaf_cell(at as u64 + 1, &record)
                })
                .collect();
            self.write_page(1, 13, &rows, None);
            let mut header = vec![0u8; HEADER_LEN];
            header[..16].copy_from_slice(MAGIC);
            let size = if self.page_size == 65536 {
                1
            } else {
                self.page_size as u16
            };
            header[16..18].copy_from_slice(&size.to_be_bytes());
            header[18] = 1;
            header[19] = 1;
            header[21] = 64;
            header[22] = 32;
            header[23] = 32;
            header[24..28].copy_from_slice(&1u32.to_be_bytes());
            header[28..32].copy_from_slice(&(self.pages.len() as u32).to_be_bytes());
            header[44..48].copy_from_slice(&4u32.to_be_bytes());
            header[56..60].copy_from_slice(&1u32.to_be_bytes());
            header[92..96].copy_from_slice(&1u32.to_be_bytes());
            header[96..100].copy_from_slice(&3_045_001u32.to_be_bytes());
            self.pages[0][..HEADER_LEN].copy_from_slice(&header);
            self.pages.concat()
        }
    }

    fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    const SQL: &str = "CREATE TABLE translation(\n  lexentry,\n  sense_num,\n  sense,\n  written_rep TEXT,\n  trans_list,\n  score\n)";

    /// A table over two levels of interior pages, with a row that overflows
    /// into two pages, rows of fewer columns, and a view and another table
    /// in the schema.
    pub(crate) fn database() -> Vec<u8> {
        let mut builder = Builder::new(512);
        let long = "долгий ".repeat(80);
        let row = |id: u64, word: &str, list: &str| {
            (
                id,
                vec![
                    text(&format!("spa/{word}")),
                    Value::Null,
                    Value::Null,
                    text(word),
                    text(list),
                    Value::Real(1.5),
                ],
            )
        };
        let first = builder.leaf(&[row(1, "banco", "банк"), row(2, "mesa", "стол | столик")]);
        let second = builder.leaf(&[row(3, "largo", &long)]);
        let third = builder.leaf(&[(
            4,
            vec![
                text("spa/x"),
                Value::Integer(-300),
                Value::Null,
                text("corto"),
            ],
        )]);
        let fourth = builder.leaf(&[(
            5,
            vec![
                Value::Blob(vec![1, 2]),
                Value::Integer(7),
                Value::Integer(1),
            ],
        )]);
        let lower = builder.interior(&[(first, 2)], second);
        let upper = builder.interior(&[(third, 4)], fourth);
        let other = builder.leaf(&[(1, vec![text("other")])]);
        let root = builder.interior(&[(lower, 3)], upper);
        builder.finish(&[
            ("table", "other", other, "CREATE TABLE other(a)"),
            (
                "view",
                "translation_grouped",
                0,
                "CREATE VIEW translation_grouped AS SELECT 1",
            ),
            ("table", "translation", root, SQL),
        ])
    }

    #[test]
    fn the_rows_of_a_table_are_read_through_interior_and_overflow_pages() {
        let bytes = database();
        let database = Database::open(&bytes).expect("open");
        let rows = database
            .rows("translation", &["written_rep", "trans_list", "sense_num"])
            .expect("rows");
        let long = "долгий ".repeat(80);
        assert_eq!(
            rows,
            [
                vec![text("banco"), text("банк"), Value::Null],
                vec![text("mesa"), text("стол | столик"), Value::Null],
                vec![text("largo"), text(&long), Value::Null],
                vec![text("corto"), Value::Null, Value::Integer(-300)],
                vec![Value::Null, Value::Null, Value::Integer(7)],
            ]
        );
        assert_eq!(
            database.rows("other", &["a"]).expect("rows"),
            [vec![text("other")]]
        );
    }

    #[test]
    fn the_values_of_a_record_are_decoded() {
        let values = vec![
            Value::Null,
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(-2),
            Value::Integer(i64::MAX),
            Value::Real(-0.25),
            text("col·lecció"),
            Value::Blob(vec![0, 255]),
        ];
        assert_eq!(record(&record_of(&values)).expect("record"), values);
        // The short integer serial types SQLite also writes.
        assert_eq!(
            record(&[4, 1, 2, 3, 0xff, 0x80, 0x00, 0x01, 0x00, 0x00]).expect("record"),
            [
                Value::Integer(-1),
                Value::Integer(-32768),
                Value::Integer(65536)
            ]
        );
        for value in [0, 1, 127, 128, 16383, 16384, 1 << 40, u64::MAX] {
            let mut bytes = Vec::new();
            put_varint(&mut bytes, value);
            assert_eq!(varint(&bytes), Some((value, bytes.len())), "{value}");
        }
    }

    #[test]
    fn the_column_names_come_from_the_create_statement() {
        assert_eq!(
            column_names(SQL).expect("columns"),
            [
                "lexentry",
                "sense_num",
                "sense",
                "written_rep",
                "trans_list",
                "score"
            ]
        );
        assert_eq!(
            column_names(
                "CREATE TABLE t(\"a b\" TEXT, [c], `d` NUMERIC(10, 2), e CHECK (e > 0), UNIQUE (a))"
            )
            .expect("columns"),
            ["a b", "c", "d", "e"]
        );
        for refused in [
            "CREATE TABLE t(id INTEGER PRIMARY KEY, a)",
            "CREATE TABLE t(a, PRIMARY KEY (a))",
            "CREATE TABLE t(a) WITHOUT ROWID",
            "CREATE TABLE t",
            "CREATE TABLE t()",
        ] {
            assert_eq!(column_names(refused), None, "{refused}");
        }
    }

    #[test]
    fn what_is_outside_the_subset_is_refused() {
        let bytes = database();
        let refused = |bytes: &[u8]| Database::open(bytes).err().map(|e| e.to_string());
        assert_eq!(
            refused(b"plain text").as_deref(),
            Some("not an SQLite database")
        );
        assert_eq!(
            refused(&bytes[..bytes.len() - 1]).as_deref(),
            Some("the file is not a whole number of pages")
        );
        let mut changed = bytes.clone();
        changed[59] = 2;
        assert_eq!(
            refused(&changed).as_deref(),
            Some("the text encoding is not UTF-8")
        );
        let mut changed = bytes.clone();
        changed[18] = 2;
        changed[19] = 2;
        assert!(refused(&changed).expect("WAL").contains("rollback"));
        let mut changed = bytes.clone();
        changed[16..18].copy_from_slice(&1000u16.to_be_bytes());
        assert!(refused(&changed).expect("page size").contains("page size"));

        let database = Database::open(&bytes).expect("open");
        let error = database.rows("missing", &["a"]).expect_err("no table");
        assert_eq!(error.to_string(), "the database has no table missing");
        let error = database
            .rows("translation", &["nothing"])
            .expect_err("no column");
        assert!(error.to_string().contains("no column nothing"), "{error}");

        // An index page where a table page should be, a page outside the
        // file, a loop, and a cut overflow chain.
        let page = |number: usize| (number - 1) * 512;
        let root = page(11);
        let mut changed = bytes.clone();
        changed[root] = 10;
        let error = Database::open(&changed)
            .expect("open")
            .rows("translation", &["sense"]);
        assert!(
            error
                .expect_err("index page")
                .to_string()
                .contains("not a table b-tree")
        );
        let mut changed = bytes.clone();
        changed[root + 8..root + 12].copy_from_slice(&999u32.to_be_bytes());
        let error = Database::open(&changed)
            .expect("open")
            .rows("translation", &["sense"]);
        assert!(
            error
                .expect_err("outside")
                .to_string()
                .contains("outside the file")
        );
        let mut changed = bytes.clone();
        changed[root + 8..root + 12].copy_from_slice(&11u32.to_be_bytes());
        let error = Database::open(&changed)
            .expect("open")
            .rows("translation", &["sense"]);
        assert!(
            error
                .expect_err("loop")
                .to_string()
                .contains("reached twice")
        );
        // The overflow pages are 4 and 5: the first points back at itself.
        let mut changed = bytes.clone();
        changed[page(4)..page(4) + 4].copy_from_slice(&4u32.to_be_bytes());
        let error = Database::open(&changed)
            .expect("open")
            .rows("translation", &["sense"]);
        assert!(
            error
                .expect_err("overflow loop")
                .to_string()
                .contains("reached twice")
        );
        let mut changed = bytes.clone();
        changed[page(4)..page(4) + 4].copy_from_slice(&0u32.to_be_bytes());
        let error = Database::open(&changed)
            .expect("open")
            .rows("translation", &["sense"]);
        assert!(
            error
                .expect_err("cut chain")
                .to_string()
                .contains("outside the file")
        );
    }
}
