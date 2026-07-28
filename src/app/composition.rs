//! What a directory or prefix is made of, grouped by file type.
//!
//! The info tab already totals a prefix's bytes and objects, which answers "how big" but not
//! "of what" — and in a bucket, "of what" is usually the question. A prefix that's 900 GB of
//! parquet with 40 MB of JSON manifests beside it is a different thing to browse, and to pay for,
//! than the same total spread across a million logs.
//!
//! Grouping is by extension, which is a guess at file type rather than a reading of it: nothing
//! here opens a file. That's the point — it costs nothing beyond the listing the info tab has
//! already fetched.

use std::collections::HashMap;

/// File types shown in their own band before the rest is folded into `other`. Five is about where
/// a stacked bar in a side pane stops being readable.
const TOP_SLICES: usize = 5;

/// Longest extension shown before it's truncated in the legend.
const MAX_LABEL: usize = 10;

/// One band: a file type and how much of the whole it accounts for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub label: String,
    pub bytes: u64,
    pub files: usize,
}

/// A prefix or directory broken down by file type, largest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composition {
    pub slices: Vec<Slice>,
    pub total_bytes: u64,
    pub files: usize,
}

/// Accumulates files one at a time, so a caller walking a directory tree (or a listing of a
/// million objects) never has to hold them all to get a breakdown.
#[derive(Default)]
pub struct Tally {
    by_type: HashMap<String, (u64, usize)>,
    total_bytes: u64,
    files: usize,
}

impl Tally {
    pub fn add(&mut self, name: &str, bytes: u64) {
        let entry = self.by_type.entry(type_label(name)).or_insert((0, 0));
        entry.0 += bytes;
        entry.1 += 1;
        self.total_bytes += bytes;
        self.files += 1;
    }

    /// Ranks the types by total bytes and folds everything past [`TOP_SLICES`] into one `other`
    /// band, so the bar keeps summing to the whole rather than quietly dropping the tail.
    pub fn finish(self) -> Option<Composition> {
        if self.files == 0 || self.total_bytes == 0 {
            return None;
        }
        let mut ranked: Vec<Slice> =
            self.by_type.into_iter().map(|(label, (bytes, files))| Slice { label, bytes, files }).collect();
        // Bytes descending, then label, so an equal-sized pair doesn't reorder between frames.
        ranked.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.label.cmp(&b.label)));

        let mut slices: Vec<Slice> = ranked.drain(..ranked.len().min(TOP_SLICES)).collect();
        if !ranked.is_empty() {
            slices.push(Slice {
                label: "other".to_string(),
                bytes: ranked.iter().map(|s| s.bytes).sum(),
                files: ranked.iter().map(|s| s.files).sum(),
            });
        }
        Some(Composition { slices, total_bytes: self.total_bytes, files: self.files })
    }
}

impl Composition {
    pub fn from_files<'a>(files: impl IntoIterator<Item = (&'a str, u64)>) -> Option<Self> {
        let mut tally = Tally::default();
        for (name, bytes) in files {
            tally.add(name, bytes);
        }
        tally.finish()
    }

    pub fn share(&self, slice: &Slice) -> f64 {
        slice.bytes as f64 / self.total_bytes as f64
    }

    /// Column widths for a stacked bar of exactly `width`, by largest remainder, so the bar always
    /// fills its space instead of drifting a column or two short from rounding.
    ///
    /// A band too small to earn a column gets none: the legend still lists it, and inflating it to
    /// a column would overstate every sliver in a prefix with a long tail.
    pub fn columns(&self, width: usize) -> Vec<usize> {
        if width == 0 || self.total_bytes == 0 {
            return vec![0; self.slices.len()];
        }
        let exact: Vec<f64> = self.slices.iter().map(|s| self.share(s) * width as f64).collect();
        let mut cols: Vec<usize> = exact.iter().map(|e| *e as usize).collect();

        let mut spare = width - cols.iter().sum::<usize>().min(width);
        let mut order: Vec<usize> = (0..cols.len()).collect();
        order.sort_by(|&a, &b| {
            let (ra, rb) = (exact[a] - exact[a].floor(), exact[b] - exact[b].floor());
            rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
        });
        for i in order {
            if spare == 0 {
                break;
            }
            cols[i] += 1;
            spare -= 1;
        }
        cols
    }
}

/// The type a file is grouped under: its lowercase extension, or `no extension`.
///
/// A leading dot doesn't make an extension (`.gitignore` is a name, not a `gitignore` file), and
/// an absurdly long one is truncated rather than allowed to push the legend around.
pub fn type_label(name: &str) -> String {
    let name = name.trim_end_matches('/');
    let stem = name.rsplit('/').next().unwrap_or(name);
    match stem.rsplit_once('.') {
        Some((head, ext)) if !head.is_empty() && !ext.is_empty() => {
            let ext = ext.to_lowercase();
            match ext.char_indices().nth(MAX_LABEL) {
                Some((cut, _)) => format!("{}…", &ext[..cut]),
                None => ext,
            }
        }
        _ => "no extension".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(c: &Composition) -> Vec<&str> {
        c.slices.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn types_come_from_the_extension() {
        assert_eq!(type_label("report.CSV"), "csv");
        assert_eq!(type_label("data/2026/part-0001.parquet"), "parquet");
        assert_eq!(type_label("archive.tar.gz"), "gz");
        assert_eq!(type_label("README"), "no extension");
        assert_eq!(type_label(".gitignore"), "no extension", "a leading dot is a name, not a type");
        assert_eq!(type_label("logs/"), "no extension");
    }

    #[test]
    fn an_absurd_extension_is_truncated() {
        assert_eq!(type_label("x.abcdefghijklmnop"), "abcdefghij…");
    }

    #[test]
    fn files_group_by_type_largest_first() {
        let c = Composition::from_files([("a.csv", 10), ("b.parquet", 100), ("c.csv", 30)]).expect("composition");

        assert_eq!(labels(&c), ["parquet", "csv"]);
        assert_eq!(c.slices[1].files, 2, "same-type files are counted together");
        assert_eq!(c.total_bytes, 140);
        assert_eq!(c.files, 3);
    }

    #[test]
    fn the_tail_beyond_the_top_types_folds_into_other() {
        let files: Vec<(String, u64)> = (0..9).map(|i| (format!("f.ext{i}"), (10 - i) as u64)).collect();
        let c = Composition::from_files(files.iter().map(|(n, b)| (n.as_str(), *b))).expect("composition");

        assert_eq!(c.slices.len(), TOP_SLICES + 1);
        assert_eq!(c.slices.last().unwrap().label, "other");
        assert_eq!(c.slices.last().unwrap().files, 4, "the four smallest types are folded together");
        assert_eq!(c.slices.iter().map(|s| s.bytes).sum::<u64>(), c.total_bytes, "the bands still sum to the whole");
    }

    #[test]
    fn nothing_to_show_has_no_composition() {
        assert!(Composition::from_files([]).is_none());
        assert!(Composition::from_files([("empty.txt", 0)]).is_none(), "zero bytes has no shape");
    }

    #[test]
    fn columns_fill_the_bar_exactly() {
        let c = Composition::from_files([("a.csv", 1), ("b.json", 1), ("c.txt", 1)]).expect("composition");

        for width in [0, 1, 7, 10, 30, 100] {
            assert_eq!(c.columns(width).iter().sum::<usize>(), width, "width {width} is filled exactly");
        }
    }

    #[test]
    fn columns_are_proportional() {
        let c = Composition::from_files([("a.csv", 75), ("b.json", 25)]).expect("composition");
        assert_eq!(c.columns(20), vec![15, 5]);
    }

    #[test]
    fn a_sliver_gets_no_column_but_stays_in_the_breakdown() {
        let c = Composition::from_files([("a.csv", 10_000), ("b.json", 1)]).expect("composition");

        assert_eq!(c.columns(10), vec![10, 0]);
        assert_eq!(c.slices.len(), 2, "it's still listed, just not drawn");
    }
}
