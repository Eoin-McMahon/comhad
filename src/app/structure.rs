//! Byte-structure maps for files the preview pane can't show.
//!
//! A binary file, or one too large to fetch, used to be a dead end: a single line saying so and
//! nothing else. Reading such a file end to end isn't an option (it can be gigabytes, and it's
//! bytes over the wire), but its *shape* is cheap: a scatter of small windows across the object,
//! each reduced to one number — Shannon entropy in bits per byte.
//!
//! That number separates the things you actually want to tell apart. Runs of zero padding sit near
//! 0, text and structured records in the middle, compressed or encrypted regions up against the
//! 8-bit ceiling. So a map of it shows where a file's header ends, whether the body is compressed,
//! whether the tail is padding — on an object no preview could ever open.
//!
//! Nothing here reads a file as *content*: entropy is a byte histogram, so a map can be shown for
//! data that must not be rendered on screen.

use std::path::Path;
use std::sync::Arc;

use crate::provider::StorageProvider;

/// Windows in a sampled map, and the bytes read per window. The product is what a map of a
/// too-large object costs to build (~192 KiB over `SAMPLE_WINDOWS` ranged requests), so both are
/// deliberately small: enough resolution to see structure at a glance, cheap enough to run every
/// time the cursor lands on a file.
pub const SAMPLE_WINDOWS: usize = 48;
pub const WINDOW_BYTES: u64 = 4096;

/// A file's shape: entropy per window in bits/byte (0..=8), in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct StructureMap {
    pub windows: Vec<f32>,
    /// Bytes that went into the map, which is not the file's size.
    pub sampled_bytes: u64,
    /// Whether the windows tile the file end to end, or are spread-out probes with unread gaps
    /// between them. Only the wording of the caption depends on it, but the distinction matters:
    /// a sampled map can miss a structure smaller than the gaps.
    pub contiguous: bool,
}

impl StructureMap {
    /// Tiles `bytes` end to end into up to [`SAMPLE_WINDOWS`] windows. For a file read in full.
    pub fn from_contiguous(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let count = SAMPLE_WINDOWS.min(bytes.len());
        let chunk = bytes.len().div_ceil(count);
        let windows = bytes.chunks(chunk).map(shannon_entropy).collect();
        Some(Self { windows, sampled_bytes: bytes.len() as u64, contiguous: true })
    }

    /// One window per probe, for windows read at spread-out offsets. Empty probes (a short read at
    /// the end of the file) are dropped rather than charted as zero-entropy, which would look like
    /// padding that isn't there.
    pub fn from_probes(probes: &[Vec<u8>]) -> Option<Self> {
        let windows: Vec<f32> = probes.iter().filter(|p| !p.is_empty()).map(|p| shannon_entropy(p)).collect();
        if windows.is_empty() {
            return None;
        }
        let sampled_bytes = probes.iter().map(|p| p.len() as u64).sum();
        Some(Self { windows, sampled_bytes, contiguous: false })
    }

    /// The map redrawn at `cols` columns: averaged down when the pane is narrower than the sample,
    /// repeated when it's wider, so the caller can render it at whatever width it has.
    pub fn resampled(&self, cols: usize) -> Vec<f32> {
        if cols == 0 || self.windows.is_empty() {
            return Vec::new();
        }
        (0..cols)
            .map(|col| {
                let start = col * self.windows.len() / cols;
                let end = ((col + 1) * self.windows.len() / cols).max(start + 1).min(self.windows.len());
                let slice = &self.windows[start..end];
                slice.iter().sum::<f32>() / slice.len() as f32
            })
            .collect()
    }

    /// How the map was obtained, for the caption under the chart. A viewer has to be able to tell
    /// a complete picture from a sampled one, or they'll read gaps as absences.
    pub fn coverage(&self) -> String {
        if self.contiguous {
            format!("{} windows over the whole file", self.windows.len())
        } else {
            format!("{} windows of {} KiB sampled across the file", self.windows.len(), WINDOW_BYTES / 1024)
        }
    }

    pub fn mean(&self) -> f32 {
        self.windows.iter().sum::<f32>() / self.windows.len() as f32
    }

    /// A plain-language read of the whole map, since the bar chart alone asks the viewer to know
    /// what entropy means. Thresholds are the usual rules of thumb: incompressible data crowds the
    /// 8-bit ceiling, natural text and code land around 4–6, and long runs of one byte sit near 0.
    pub fn verdict(&self) -> &'static str {
        let mean = self.mean();
        let spread = self.windows.iter().cloned().fold(f32::MIN, f32::max)
            - self.windows.iter().cloned().fold(f32::MAX, f32::min);
        match mean {
            m if m < 1.0 => "mostly empty or padded",
            m if m < 4.0 => "structured, low entropy",
            m if m >= 7.5 && spread < 1.0 => "uniformly compressed or encrypted",
            m if m >= 7.0 => "mostly compressed",
            _ => "mixed: headers and payload",
        }
    }
}

/// Shannon entropy of `bytes` in bits per byte, 0 (all one value) to 8 (flat distribution).
fn shannon_entropy(bytes: &[u8]) -> f32 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f32;
    -counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f32 / len;
            p * p.log2()
        })
        .sum::<f32>()
}

/// Byte offsets of `windows` probes spread evenly across a file of `size`, first at the very start
/// and last ending at EOF, so a map always covers the header and the tail — the two places file
/// formats put something worth seeing.
pub fn window_offsets(size: u64, windows: usize, window_bytes: u64) -> Vec<u64> {
    let last = size.saturating_sub(window_bytes);
    if windows <= 1 || last == 0 {
        return vec![0];
    }
    (0..windows).map(|i| last * i as u64 / (windows as u64 - 1)).collect()
}

/// Samples a local file. Files small enough to read outright are mapped contiguously, since a
/// complete map beats a sampled one and costs a single read.
pub fn sample_local(path: &Path, size: u64) -> Option<StructureMap> {
    use std::io::{Seek, SeekFrom};

    let budget = SAMPLE_WINDOWS as u64 * WINDOW_BYTES;
    if size <= budget {
        return StructureMap::from_contiguous(&std::fs::read(path).ok()?);
    }

    let mut file = std::fs::File::open(path).ok()?;
    let mut probes = Vec::with_capacity(SAMPLE_WINDOWS);
    for offset in window_offsets(size, SAMPLE_WINDOWS, WINDOW_BYTES) {
        if file.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
        let mut buf = vec![0u8; WINDOW_BYTES as usize];
        let read = read_fully(&mut file, &mut buf);
        buf.truncate(read);
        probes.push(buf);
    }
    StructureMap::from_probes(&probes)
}

/// Fills `buf` as far as the file allows, returning how many bytes landed. A single `read` may
/// return short for reasons that aren't EOF, which would understate a window.
fn read_fully(file: &mut std::fs::File, buf: &mut [u8]) -> usize {
    use std::io::Read;
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => total += n,
        }
    }
    total
}

/// Remote counterpart to [`sample_local`]: one ranged request per window. A failed window is
/// skipped rather than failing the map, so a map still appears if a few requests are refused.
pub async fn sample_remote(
    client: &Arc<dyn StorageProvider>,
    container: &str,
    key: &str,
    size: u64,
) -> Option<StructureMap> {
    if size <= WINDOW_BYTES {
        return StructureMap::from_contiguous(&client.read_range(container, key, size).await.ok()?);
    }
    let mut probes = Vec::with_capacity(SAMPLE_WINDOWS);
    for offset in window_offsets(size, SAMPLE_WINDOWS, WINDOW_BYTES) {
        if let Ok(bytes) = client.read_window(container, key, offset, WINDOW_BYTES).await {
            probes.push(bytes);
        }
    }
    StructureMap::from_probes(&probes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_of_a_single_repeated_byte_is_zero() {
        assert_eq!(shannon_entropy(&[0u8; 4096]), 0.0);
        assert_eq!(shannon_entropy(&[0xff; 100]), 0.0);
    }

    #[test]
    fn entropy_of_every_byte_value_is_the_eight_bit_ceiling() {
        let flat: Vec<u8> = (0..=255).collect();
        assert!((shannon_entropy(&flat) - 8.0).abs() < 1e-5);
    }

    #[test]
    fn entropy_of_ascii_text_sits_in_the_middle() {
        let text = "the quick brown fox jumps over the lazy dog, ".repeat(40);
        let entropy = shannon_entropy(text.as_bytes());
        assert!((3.0..6.0).contains(&entropy), "text was {entropy} bits/byte");
    }

    #[test]
    fn a_contiguous_map_tiles_the_whole_input() {
        let map = StructureMap::from_contiguous(&[0u8; 4096]).expect("map");
        assert_eq!(map.windows.len(), SAMPLE_WINDOWS);
        assert_eq!(map.sampled_bytes, 4096);
        assert!(map.contiguous);
    }

    #[test]
    fn a_tiny_input_gets_one_window_per_byte_at_most() {
        let map = StructureMap::from_contiguous(b"hello").expect("map");
        assert_eq!(map.windows.len(), 5);
    }

    #[test]
    fn an_empty_input_has_no_map() {
        assert!(StructureMap::from_contiguous(&[]).is_none());
        assert!(StructureMap::from_probes(&[Vec::new(), Vec::new()]).is_none());
    }

    #[test]
    fn probes_skip_empty_windows_rather_than_charting_them_as_padding() {
        let map = StructureMap::from_probes(&[vec![7u8; 16], Vec::new(), vec![9u8; 16]]).expect("map");
        assert_eq!(map.windows.len(), 2, "the short read at EOF is dropped");
        assert_eq!(map.sampled_bytes, 32);
        assert!(!map.contiguous);
    }

    #[test]
    fn window_offsets_span_the_file_from_start_to_eof() {
        let offsets = window_offsets(1_000_000, 4, 4096);
        assert_eq!(offsets.first(), Some(&0), "the header is always sampled");
        assert_eq!(offsets.last(), Some(&(1_000_000 - 4096)), "the last window ends at EOF");
        assert!(offsets.windows(2).all(|w| w[0] < w[1]), "offsets advance");
    }

    #[test]
    fn window_offsets_collapse_when_the_file_is_barely_bigger_than_one_window() {
        assert_eq!(window_offsets(4096, 48, 4096), vec![0]);
        assert_eq!(window_offsets(10, 48, 4096), vec![0]);
    }

    #[test]
    fn resampling_averages_down_and_stretches_up() {
        let map = StructureMap { windows: vec![0.0, 0.0, 8.0, 8.0], sampled_bytes: 4, contiguous: true };

        assert_eq!(map.resampled(2), vec![0.0, 8.0], "narrow panes average neighbours");
        assert_eq!(map.resampled(4), map.windows, "an exact fit is unchanged");
        assert_eq!(map.resampled(8).len(), 8, "wide panes stretch");
        assert!(map.resampled(0).is_empty());
    }

    #[test]
    fn verdicts_name_what_the_shape_means() {
        let of = |windows: Vec<f32>| StructureMap { windows, sampled_bytes: 0, contiguous: true }.verdict();

        assert_eq!(of(vec![0.0; 8]), "mostly empty or padded");
        assert_eq!(of(vec![2.5; 8]), "structured, low entropy");
        assert_eq!(of(vec![7.99; 8]), "uniformly compressed or encrypted");
        assert_eq!(of(vec![5.0, 5.5, 6.0]), "mixed: headers and payload");
    }

    #[test]
    fn sampling_a_local_file_smaller_than_the_budget_reads_it_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("small.bin");
        std::fs::write(&path, vec![0u8; 8192]).unwrap();

        let map = sample_local(&path, 8192).expect("map");
        assert!(map.contiguous);
        assert_eq!(map.sampled_bytes, 8192);
    }

    #[test]
    fn sampling_a_large_local_file_probes_windows_across_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.bin");
        // Zeroed head, high-entropy tail: the map should see both.
        let mut bytes = vec![0u8; 400_000];
        bytes.extend((0..400_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
        std::fs::write(&path, &bytes).unwrap();

        let map = sample_local(&path, bytes.len() as u64).expect("map");
        assert!(!map.contiguous);
        assert_eq!(map.windows.len(), SAMPLE_WINDOWS);
        assert!(map.windows[0] < 1.0, "the zeroed head reads as empty");
        assert!(map.windows[SAMPLE_WINDOWS - 1] > 6.0, "the noisy tail reads as dense");
    }
}
