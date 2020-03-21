use crate::AnsiColorChoice;
use crate::{logger, rust};

use anyhow::{anyhow, bail, ensure, Context as _};
use cargo_metadata::{Package, Resolve, Target};
use itertools::Itertools as _;
use log::info;
use serde::Deserialize;
use toml_edit::Document;

use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn create_workspace(dir: impl AsRef<Path>, dry_run: bool) -> anyhow::Result<()> {
    let dir = dir.as_ref();
    crate::fs::create_dir_all(dir, dry_run)?;
    crate::fs::write(dir.join("Cargo.toml"), CARGO_TOML, dry_run)?;
    info!("Created a new workspace: {}", dir.display());
    return Ok(());

    static CARGO_TOML: &str = r#"[workspace]
members = []
exclude = []
"#;
}

pub(crate) fn cargo_metadata_no_deps(
    manifest_path: Option<&Path>,
    color: AnsiColorChoice,
    cwd: &Path,
) -> anyhow::Result<cargo_metadata::Metadata> {
    let program = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut args = vec![
        "metadata".into(),
        "--no-deps".into(),
        "--format-version".into(),
        "1".into(),
        "--color".into(),
        <&str>::from(color).into(),
        "--frozen".into(),
    ];
    if let Some(cli_option_manifest_path) = manifest_path {
        args.push("--manifest-path".into());
        args.push(cwd.join(cli_option_manifest_path).into_os_string());
    }

    let metadata = crate::process::cmd(program, args).dir(cwd).read()?;
    let metadata = serde_json::from_str::<cargo_metadata::Metadata>(&metadata)?;

    if metadata
        .resolve
        .as_ref()
        .map_or(false, |Resolve { root, .. }| root.is_some())
    {
        bail!("the target package must be a virtual manifest");
    }
    Ok(metadata)
}

pub(crate) fn raise_unless_virtual(workspace_root: &Path) -> anyhow::Result<()> {
    let manifest_path = workspace_root.join("Cargo.toml");
    let CargoToml { package } = crate::fs::read_toml(&manifest_path)?;
    if package.is_some() {
        bail!(
            "the target manifest must be a virtual one: {}",
            manifest_path.display(),
        );
    }
    Ok(())
}

pub(crate) fn add_member(
    metadata: &cargo_metadata::Metadata,
    cargo_toml: &str,
    bin: &str,
    bin_name: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<String> {
    let CargoTomlPackage { name, .. } = toml::from_str::<CargoToml>(cargo_toml)
        .with_context(|| "failed to parse the manifest")?
        .package
        .with_context(|| "`package.name` is missing")?;

    let manifest_path = if let Some(package) = metadata
        .packages
        .iter()
        .find(|p| metadata.workspace_members.contains(&p.id) && p.name == name)
    {
        info!(
            "`{}` already exists: {}",
            name,
            metadata.workspace_root.display(),
        );
        package.manifest_path.clone()
    } else {
        let package_dir = metadata.workspace_root.join(&name);
        ensure!(!package_dir.exists(), "{} exists", package_dir.display());
        modify_members(
            &metadata.workspace_root,
            Some(&package_dir),
            None,
            None,
            Some(&package_dir),
            dry_run,
        )?;
        package_dir.join("Cargo.toml")
    };

    let bin_path = if let Some(bin_name) = bin_name {
        manifest_path
            .with_file_name("src")
            .join("bin")
            .join(bin_name)
            .with_extension("rs")
    } else {
        manifest_path.with_file_name("src").join("main.rs")
    };

    crate::fs::create_dir_all(bin_path.parent().expect("should not empty"), dry_run)?;
    write_unless_up_to_date(&manifest_path, cargo_toml, dry_run)?;
    write_unless_up_to_date(&bin_path, bin, dry_run)?;

    return Ok(name);

    fn write_unless_up_to_date(path: &Path, content: &str, dry_run: bool) -> anyhow::Result<()> {
        if path.exists() && crate::fs::read(path)? == content {
            info!("{} is up to date", path.display());
            Ok(())
        } else {
            crate::fs::write(path, content, dry_run)
        }
    }
}

pub(crate) fn modify_package_name(cargo_toml: &mut Document, name: &str) -> anyhow::Result<()> {
    let old_name = cargo_toml["package"]["name"]
        .as_str()
        .with_context(|| "`package.name` must be a string")?
        .to_owned();

    cargo_toml["package"]["name"] = toml_edit::value(name);
    info!("`package.name`: {:?} → {:?}", old_name, name);
    Ok(())
}

pub(crate) fn modify_package_version(cargo_toml: &mut Document, version: &str) {
    info!(
        "`package.version`: {:?} → {:?}",
        cargo_toml["version"].as_str(),
        version,
    );
    cargo_toml["package"]["version"] = toml_edit::value(version)
}

pub(crate) fn modify_package_publish(cargo_toml: &mut Document, publish: bool) {
    info!(
        "`package.publish`: {:?} → {}",
        cargo_toml["publish"].as_bool(),
        publish,
    );
    cargo_toml["package"]["publish"] = toml_edit::value(publish)
}

pub(crate) fn modify_members<'a>(
    workspace_root: &Path,
    add_to_workspace_members: Option<&'a Path>,
    add_to_workspace_exclude: Option<&'a Path>,
    rm_from_workspace_members: Option<&'a Path>,
    rm_from_workspace_exclude: Option<&'a Path>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let manifest_path = workspace_root.join("Cargo.toml");
    let mut cargo_toml = crate::fs::read_toml_edit(&manifest_path)?;

    for (param, add, rm) in &[
        (
            "members",
            add_to_workspace_members,
            rm_from_workspace_members,
        ),
        (
            "exclude",
            add_to_workspace_exclude,
            rm_from_workspace_exclude,
        ),
    ] {
        let relative_to_root = |path: &'a Path| -> _ {
            let path = path.strip_prefix(workspace_root).unwrap_or(path);
            path.to_str()
                .with_context(|| format!("{:?} is not valid UTF-8 path", path))
        };

        let same_paths = |value: &toml_edit::Value, target: &str| -> _ {
            value.as_str().map_or(false, |s| {
                workspace_root.join(s) == workspace_root.join(target)
            })
        };

        let array = cargo_toml["workspace"][param]
            .or_insert(toml_edit::value(toml_edit::Array::default()))
            .as_array_mut()
            .with_context(|| format!("`workspace.{}` must be an array", param))?;
        if let Some(add) = *add {
            let add = relative_to_root(add)?;
            if !dry_run && array.iter().all(|m| !same_paths(m, add)) {
                array.push(add);
            }
            info!("Added to {:?} to `workspace.{}`", add, param);
        }
        if let Some(rm) = rm {
            let rm = relative_to_root(rm)?;
            if !dry_run {
                let i = array.iter().position(|m| same_paths(m, rm));
                if let Some(i) = i {
                    array.remove(i);
                }
            }
            info!("Removed {:?} from `workspace.{}`", rm, param);
        }
    }

    crate::fs::write(&manifest_path, cargo_toml.to_string(), dry_run)?;
    Ok(())
}

pub(crate) fn import_script(
    workspace_root: &Path,
    script: &str,
    dry_run: bool,
    str_width: fn(&str) -> usize,
    path: impl FnOnce(&str) -> PathBuf,
) -> anyhow::Result<String> {
    let (main_rs, cargo_toml) = rust::replace_cargo_lang_code_with_default(script)?;

    let package_name = toml::from_str::<CargoToml>(&cargo_toml)
        .with_context(|| "failed to parse the manifest")?
        .package
        .with_context(|| "missing `package.name`")?
        .name;

    let path = path(&package_name);

    let prev_cargo_toml = prev_content(&path.join("Cargo.toml"))?;
    let prev_main_rs = prev_content(&path.join("src").join("main.rs"))?;

    crate::fs::create_dir_all(&path, dry_run)?;
    crate::fs::write(path.join("Cargo.toml"), &cargo_toml, dry_run)?;

    crate::fs::create_dir_all(path.join("src"), dry_run)?;
    crate::fs::write(path.join("src").join("main.rs"), &main_rs, dry_run)?;

    modify_members(&workspace_root, Some(&*path), None, None, None, dry_run)?;

    logger::info_diff(
        &prev_cargo_toml,
        &cargo_toml,
        path.join("Cargo.toml").display(),
        str_width,
    );

    logger::info_diff(
        &prev_main_rs,
        &main_rs,
        path.join("src").join("main.rs").display(),
        str_width,
    );

    return Ok(package_name);

    fn prev_content(path: &Path) -> anyhow::Result<String> {
        if path.exists() {
            crate::fs::read(path)
        } else {
            Ok("".to_owned())
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CargoToml {
    #[serde(default)]
    package: Option<CargoTomlPackage>,
}

#[derive(Deserialize)]
struct CargoTomlPackage {
    name: String,
    #[serde(default)]
    default_run: Option<String>,
}

pub(crate) trait MetadataExt {
    fn find_package(&self, name: &str) -> anyhow::Result<&Package>;
}

impl MetadataExt for cargo_metadata::Metadata {
    fn find_package(&self, name: &str) -> anyhow::Result<&Package> {
        self.packages
            .iter()
            .find(|p| p.name == name)
            .with_context(|| format!("no such package: {:?}", name))
    }
}

pub(crate) trait PackageExt {
    fn find_default_bin(&self) -> anyhow::Result<(&Path, String)>;
}

impl PackageExt for Package {
    fn find_default_bin(&self) -> anyhow::Result<(&Path, String)> {
        let (cargo_toml_str, cargo_toml_value) =
            crate::fs::read_toml_with_raw::<_, CargoToml>(&self.manifest_path)?;
        let default_run = cargo_toml_value
            .package
            .as_ref()
            .and_then(|CargoTomlPackage { default_run, .. }| default_run.as_ref());

        let Target { src_path, .. } = self
            .targets
            .iter()
            .filter(|Target { kind, name, .. }| {
                kind.contains(&"bin".to_owned()) && default_run.map_or(true, |d| d == name)
            })
            .exactly_one()
            .map_err(|err| match err.count() {
                0 => anyhow!("no `bin` targets found"),
                _ => anyhow!("could not determine which `bin` target to export"),
            })?;

        Ok((src_path, cargo_toml_str))
    }
}
