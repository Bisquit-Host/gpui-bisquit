pub(crate) fn upload_extent(width: u32, x: u32, texels: u64) -> (u32, u32) {
    if x == 0 && texels >= u64::from(width) {
        (width, (texels / u64::from(width)) as u32)
    } else {
        (texels.min(u64::from(width - x)) as u32, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::upload_extent;

    #[test]
    fn group_rows_without_crossing_partial_rows() {
        assert_eq!(upload_extent(16, 0, 53), (16, 3));
        assert_eq!(upload_extent(16, 5, 53), (11, 1));
        assert_eq!(upload_extent(16, 0, 5), (5, 1));
        assert_eq!(upload_extent(16, 5, 0), (0, 1));
        for width in 1..64 {
            for start in 0..width {
                for count in [0, 1, width - 1, width, width + 1, width * 12 + 3] {
                    let mut remaining = u64::from(count);
                    let mut offset = u64::from(start);
                    while remaining > 0 {
                        let x = (offset % u64::from(width)) as u32;
                        let (w, h) = upload_extent(width, x, remaining);
                        let written = u64::from(w) * u64::from(h);
                        assert!(written > 0 && written <= remaining);
                        assert!(x + w <= width);
                        assert!(h == 1 || x == 0 && w == width);
                        remaining -= written;
                        offset += written;
                    }
                    assert_eq!(offset, u64::from(start + count));
                }
            }
        }
    }
}
