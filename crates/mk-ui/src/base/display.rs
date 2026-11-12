//! Text sizes (Settings → Display, revision 7).

/// Text size steps in percent.
pub(crate) const STEPS: [u32; 7] = [90, 100, 115, 130, 150, 175, 200];

pub(crate) const DEFAULT_SIZE: u32 = 100;

/// A configured size kept within 90–200 %.
pub fn clamp_size(percent: u32) -> u32 {
    percent.clamp(90, 200)
}

pub(crate) fn larger_size(p: u32) -> u32 {
    larger(p)
}

pub(crate) fn smaller_size(p: u32) -> u32 {
    smaller(p)
}

fn larger(p: u32) -> u32 {
    STEPS.iter().copied().find(|s| *s > p).unwrap_or(200)
}

fn smaller(p: u32) -> u32 {
    STEPS.iter().rev().copied().find(|s| *s < p).unwrap_or(90)
}
