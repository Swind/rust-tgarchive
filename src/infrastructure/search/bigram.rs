use super::normalize::normalize_text;

/// Han ideographs (BMP and supplementary planes).
pub fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0x2CEB0..=0x2EBEF
            | 0x30000..=0x3134F
    )
}

/// Contiguous CJK runs of already-normalized text.
pub(super) fn cjk_runs(normalized: &str) -> Vec<Vec<char>> {
    let mut runs = Vec::new();
    let mut current = Vec::new();
    for ch in normalized.chars() {
        if is_cjk(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

pub(super) fn run_bigrams(run: &[char]) -> impl Iterator<Item = String> + '_ {
    run.windows(2).map(|pair| pair.iter().collect())
}

/// Overlapping character pairs inside each CJK run. Single-character runs produce nothing:
/// the index stores no unigrams (single-character queries use a `LIKE` scan).
pub fn cjk_bigrams(text: &str) -> Vec<String> {
    cjk_runs(&normalize_text(text))
        .iter()
        .flat_map(|run| run_bigrams(run))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_stay_inside_cjk_runs() {
        assert_eq!(cjk_bigrams("台北咖啡"), ["台北", "北咖", "咖啡"]);
        assert_eq!(
            cjk_bigrams("台北 GitLab Runner 咖啡"),
            ["台北", "咖啡"],
            "no cross-boundary pairs"
        );
        assert_eq!(cjk_bigrams("北 a 台"), Vec::<String>::new());
        assert_eq!(
            cjk_bigrams("我，你"),
            Vec::<String>::new(),
            "punctuation splits runs"
        );
    }

    #[test]
    fn handles_supplementary_plane_characters() {
        assert_eq!(cjk_bigrams("𠀀𠀁𠀂"), ["𠀀𠀁", "𠀁𠀂"]);
    }
}
