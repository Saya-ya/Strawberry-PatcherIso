use std::{fs, path::Path};

use anyhow::{Context, Result};

pub fn copy_file_creating_parent(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<u64> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_file_and_creates_parent() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("source.bin");
        let dst = temp.path().join("nested/out.bin");
        fs::write(&src, b"abc").unwrap();

        let copied = copy_file_creating_parent(&src, &dst).unwrap();
        assert_eq!(copied, 3);
        assert_eq!(fs::read(&dst).unwrap(), b"abc");
    }
}
