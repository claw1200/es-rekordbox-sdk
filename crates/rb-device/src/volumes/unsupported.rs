//! Platforms with no volume support: nothing is listed, so nothing can be ejected or erased.

use std::path::Path;

use super::Mounted;
use crate::{fail, Result};

pub(super) fn mounted() -> Vec<Mounted> {
    Vec::new()
}

pub(super) fn eject(_volume: &Path) -> Result<()> {
    fail("Ejecting isn’t supported on this platform")
}

pub(super) fn erase_as_fat32(_volume: &Path, _label: &str) -> Result<()> {
    fail("Formatting isn’t supported on this platform")
}
