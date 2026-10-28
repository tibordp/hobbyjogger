//! SD card listings: replies to `ls -s <dir>`.
//!
//! The machine answers in `LOAD_INFO` frames holding CRLF-separated `name size YYYYMMDDhhmmss`
//! entries (several per frame), then a `LOAD_FINISH`. Directory names end in `/`. Spaces and other
//! special characters in names are escaped as in [`escape_arg`](crate::command::escape_arg).
//! A directory that doesn't exist lists as empty.
//!
//! `rm <path>` replies with `LOAD_FINISH` `ok` on success and `LOAD_ERROR`
//! `Could not delete <path>` on failure.

use crate::command::unescape_arg;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// `YYYYMMDDhhmmss` split into fields. Files written while the machine's clock isn't set are
    /// dated 1980-01-01; those are reported as `None`.
    pub modified: Option<[u16; 6]>,
}

/// Parse one line of a listing. Returns `None` for blank or malformed lines.
pub fn parse_entry(line: &str) -> Option<Entry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut parts = line.rsplitn(3, ' ');
    let date = parts.next()?;
    let size = parts.next()?.parse().ok()?;
    let name = parts.next()?.trim_end();
    if name.is_empty() || date.len() != 14 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let field = |r: std::ops::Range<usize>| date[r].parse::<u16>().ok();
    let modified = [
        field(0..4)?,
        field(4..6)?,
        field(6..8)?,
        field(8..10)?,
        field(10..12)?,
        field(12..14)?,
    ];
    let (name, dir) = match name.strip_suffix('/') {
        Some(n) => (n, true),
        None => (name, false),
    };
    Some(Entry {
        name: unescape_arg(name),
        dir,
        size,
        modified: (modified[0] > 1980).then_some(modified),
    })
}

/// Assembles a listing from `LOAD_INFO` payloads, which may split a line across frames.
#[derive(Default)]
pub struct Listing {
    partial: String,
    pub entries: Vec<Entry>,
}

impl Listing {
    pub fn feed(&mut self, payload: &[u8]) {
        self.partial.push_str(&String::from_utf8_lossy(payload));
        while let Some(i) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=i).collect();
            self.entries.extend(parse_entry(&line));
        }
    }

    pub fn finish(mut self) -> Vec<Entry> {
        let rest = std::mem::take(&mut self.partial);
        self.entries.extend(parse_entry(&rest));
        self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_captured_listing() {
        let mut l = Listing::default();
        // From docs/research/captures/z1-sd-listing.jsonl; the second frame splits a line.
        l.feed(b".md5/ 0 20260328161406\r\ngoto-pack-pos-z1.nc  54 20260510102126\r\nhj-air");
        l.feed(b"-test.nc  3611 19800101003322\r\nSystem\x01Volume\x01Information/ 0 20251101155556\r\n");
        let e = l.finish();
        assert_eq!(e.len(), 4);
        assert_eq!(
            e[0],
            Entry {
                name: ".md5".into(),
                dir: true,
                size: 0,
                modified: Some([2026, 3, 28, 16, 14, 6])
            }
        );
        assert_eq!(e[1].name, "goto-pack-pos-z1.nc");
        assert_eq!(e[1].size, 54);
        assert!(!e[1].dir);
        assert_eq!(e[2].name, "hj-air-test.nc");
        assert_eq!(e[2].modified, None);
        assert_eq!(e[3].name, "System Volume Information");
        assert!(e[3].dir);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_entry("Load directory finished."), None);
        assert_eq!(parse_entry(""), None);
    }
}
