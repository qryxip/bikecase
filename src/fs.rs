use anyhow::{anyhow, Context as _};
use log::info;
use serde::de::DeserializeOwned;

use std::io;
use std::path::Path;

pub(crate) fn read(path: impl AsRef<Path>) -> anyhow::Result<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).map_err(|err| match err.kind() {
        io::ErrorKind::InvalidData => anyhow!("path at `{}` was not valid utf-8"),
        _ => anyhow::Error::new(err).context(format!("failed to read {}", path.display())),
    })
}

pub(crate) fn read_toml<P: AsRef<Path>, T: DeserializeOwned>(path: P) -> anyhow::Result<T> {
    let (_, value) = read_toml_with_raw(path)?;
    Ok(value)
}

pub(crate) fn read_toml_with_raw<P: AsRef<Path>, T: DeserializeOwned>(
    path: P,
) -> anyhow::Result<(String, T)> {
    let path = path.as_ref();
    let string = read(path)?;
    let value = toml::from_str(&string)
        .with_context(|| format!("failed to parse the TOML file at {}", path.display()))?;
    Ok((string, value))
}

pub(crate) fn read_toml_edit(path: impl AsRef<Path>) -> anyhow::Result<toml_edit::Document> {
    let path = path.as_ref();
    read(path)?
        .parse()
        .with_context(|| format!("failed to parse the TOML file at {}", path.display()))
}

pub(crate) fn write(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let path = path.as_ref();
    if !dry_run {
        std::fs::write(path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    info!(
        "{}Wrote {}",
        if dry_run { "[dry-run] " } else { "" },
        path.display(),
    );
    Ok(())
}

pub(crate) fn copy(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let (src, dst) = (src.as_ref(), dst.as_ref());
    if !dry_run {
        std::fs::copy(src, dst).with_context(|| {
            format!("failed to copy `{}` to `{}`", src.display(), dst.display())
        })?;
    }
    info!(
        "{}Copied {} to {}",
        if dry_run { "[dry-run] " } else { "" },
        src.display(),
        dst.display(),
    );
    Ok(())
}

pub(crate) fn create_dir_all(path: impl AsRef<Path>, dry_run: bool) -> anyhow::Result<()> {
    let path = path.as_ref();
    if !dry_run {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create directory `{}`", path.display()))?;
    }
    Ok(())
}

pub(crate) fn remove_dir_all(path: impl AsRef<Path>, dry_run: bool) -> anyhow::Result<()> {
    let path = path.as_ref();
    if !dry_run {
        remove_dir_all::remove_dir_all(path)
            .with_context(|| format!("failed to remove `{}`", path.display()))?;
    }
    info!(
        "{}Removed {}",
        if dry_run { "[dry-run] " } else { "" },
        path.display(),
    );
    Ok(())
}
