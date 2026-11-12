//! The brush mask of an engraved book picture (app-desktop.md revision 40): 256 × 256
//! marks, one per pixel — 0 automatic, 1 keep (always subject), 2 remove (always
//! background) — stored as runs: pairs of bytes (mark, run length − 1), row by row.
//! A plain format both the app and the commands read without a compression library.

/// The mask's side in pixels.
pub const SIDE: usize = 256;
/// Marks in a mask.
pub const LEN: usize = SIDE * SIDE;

/// The runs of a mask (empty when every mark is automatic).
pub fn pack(mask: &[u8]) -> Vec<u8> {
    if mask.iter().all(|m| *m == 0) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut it = mask.iter().copied().peekable();
    while let Some(m) = it.next() {
        let mut run = 0u8;
        while run < u8::MAX && it.peek() == Some(&m) {
            it.next();
            run += 1;
        }
        out.push(m);
        out.push(run);
    }
    out
}

/// A mask from its runs; `None` when the runs do not make exactly [`LEN`] marks of
/// 0, 1 or 2.
pub fn unpack(runs: &[u8]) -> Option<Vec<u8>> {
    if !runs.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(LEN);
    for pair in runs.chunks_exact(2) {
        let (m, run) = match pair {
            [m, run] => (*m, usize::from(*run) + 1),
            _ => return None,
        };
        if m > 2 || out.len() + run > LEN {
            return None;
        }
        out.extend(std::iter::repeat_n(m, run));
    }
    (out.len() == LEN).then_some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn round_trip() {
        let mut m = vec![0u8; LEN];
        for (i, v) in m.iter_mut().enumerate().skip(1000).take(3000) {
            *v = if i % 700 < 300 { 1 } else { 2 };
        }
        let p = pack(&m);
        assert!(p.len() < 700, "{}", p.len());
        assert_eq!(unpack(&p).unwrap(), m);
        assert!(pack(&vec![0; LEN]).is_empty());
        assert!(unpack(&[3, 0]).is_none());
        assert!(unpack(&[0, 255]).is_none(), "too short");
    }
}
