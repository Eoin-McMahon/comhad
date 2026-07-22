//! Fuzzy substring matching for `/`'s filter: every query character must appear in the
//! candidate, case-insensitively, in order but not necessarily contiguous (so `hlo` matches
//! `hello.csv`), same relationship as fzf/Sublime's "quick open".

/// Returns the matched char indices in `haystack` (one per `needle` char, in order) if every
/// character of `needle` appears in order, `None` otherwise. Empty `needle` matches nothing, 
/// callers treat "no filter" as "show everything" before reaching this.
pub fn fuzzy_positions(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    if needle.is_empty() {
        return None;
    }
    let hay: Vec<char> = haystack.chars().collect();
    let mut positions = Vec::with_capacity(needle.chars().count());
    let mut hi = 0;
    for nc in needle.chars() {
        let nc = nc.to_lowercase().next().unwrap_or(nc);
        let found = (hi..hay.len()).find(|&i| hay[i].to_lowercase().next().unwrap_or(hay[i]) == nc)?;
        positions.push(found);
        hi = found + 1;
    }
    Some(positions)
}

pub fn fuzzy_matches(haystack: &str, needle: &str) -> bool {
    fuzzy_positions(haystack, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_subsequence() {
        assert!(fuzzy_matches("hello.csv", "hlo"));
        assert!(fuzzy_matches("hello.csv", "HELLO"));
        assert!(fuzzy_matches("archive/2024/hello.csv", "a24hello"));
    }

    #[test]
    fn rejects_out_of_order_or_missing() {
        assert!(!fuzzy_matches("hello.csv", "olh"));
        assert!(!fuzzy_matches("hello.csv", "z"));
    }

    #[test]
    fn positions_point_at_the_matched_chars() {
        assert_eq!(fuzzy_positions("hello.csv", "hlo"), Some(vec![0, 2, 4]));
    }
}
