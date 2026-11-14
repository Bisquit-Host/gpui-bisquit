/// Android offsets/counts use UTF-16 units. Empty text must not subtract one
/// from an unsigned length, even when overflow checks are enabled.
pub(crate) fn text_change(old: &str, new: &str) -> Option<(i32, i32, i32)> {
    let old: Vec<_> = old.encode_utf16().collect();
    let new: Vec<_> = new.encode_utf16().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = old.len() - prefix - suffix;
    let added = new.len() - prefix - suffix;
    (removed != 0 || added != 0).then_some((prefix as i32, removed as i32, added as i32))
}

#[cfg(test)]
mod tests {
    use super::text_change;

    #[test]
    fn empty_text_insertions_and_deletions_have_valid_counts() {
        assert_eq!(text_change("", "hello"), Some((0, 0, 5)));
        assert_eq!(text_change("hello", ""), Some((0, 5, 0)));
        assert_eq!(text_change("", ""), None);
    }

    #[test]
    fn text_changes_use_utf16_and_exclude_common_suffixes() {
        assert_eq!(text_change("😀end", "😀new end"), Some((2, 0, 4)));
        assert_eq!(text_change("a😀z", "az"), Some((1, 2, 0)));
        assert_eq!(text_change("a😀z", "a😃z"), Some((2, 1, 1)));
        assert_eq!(text_change("same", "same"), None);
    }
}
