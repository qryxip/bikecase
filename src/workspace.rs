use crate::AnsiColorChoice;

use anyhow::{anyhow, bail, ensure, Context as _};
use cargo_metadata::{Package, Resolve, Target};
use if_chain::if_chain;
use itertools::Itertools as _;
use log::info;
use serde::Deserialize;
use syn::{Lit, Meta, MetaNameValue};
use toml_edit::Document;

use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::{env, iter};

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
    path: impl FnOnce(&str) -> PathBuf,
) -> anyhow::Result<String> {
    let (main_rs, cargo_toml) = replace_cargo_lang_code_with_default(script)?;

    let CargoTomlPackage {
        name: package_name, ..
    } = toml::from_str::<CargoToml>(&cargo_toml)
        .with_context(|| "failed to parse the manifest")?
        .package
        .with_context(|| "missing `package.name`")?;
    let path = path(&package_name);

    crate::fs::create_dir_all(&path, dry_run)?;
    crate::fs::write(path.join("Cargo.toml"), cargo_toml, dry_run)?;

    crate::fs::create_dir_all(path.join("src"), dry_run)?;
    crate::fs::write(path.join("src").join("main.rs"), main_rs, dry_run)?;

    modify_members(&workspace_root, Some(&*path), None, None, None, dry_run)?;
    Ok(package_name)
}

fn replace_cargo_lang_code_with_default(code: &str) -> anyhow::Result<(String, String)> {
    return replace_cargo_lang_code(code, MANIFEST, || {
        anyhow!("could not find the `cargo` code block")
    });

    static MANIFEST: &str = "# Leave blank.";
}

fn replace_cargo_lang_code(
    code: &str,
    with: &str,
    on_not_found: impl FnOnce() -> anyhow::Error,
) -> anyhow::Result<(String, String)> {
    let mut code_lines = code.lines().map(Cow::from).map(Some).collect::<Vec<_>>();

    let syn::File { shebang, attrs, .. } = syn::parse_file(code)?;
    if shebang.is_some() {
        code_lines[0] = None;
    }

    let mut remove = |i: usize, start: _, end: Option<_>| {
        let entry = &mut code_lines[i];
        if let Some(line) = entry {
            let first = &line[..start];
            let second = match end {
                Some(end) if end < line.len() => &line[end..],
                _ => "",
            };
            *line = format!("{}{}", first, second).into();
            if line.is_empty() {
                *entry = None;
            }
        }
    };

    let mut doc = "".to_owned();

    for attr in attrs {
        if_chain! {
            if let Ok(meta) = attr.parse_meta();
            if let Meta::NameValue(MetaNameValue { path, lit, .. }) = meta;
            if path.get_ident().map_or(false, |i| i == "doc");
            if let Lit::Str(lit_str) = lit;
            then {
                doc += lit_str.value().trim_start_matches(' ');
                doc += "\n";

                for tt in attr.tokens {
                    let (start, end) = (tt.span().start(), tt.span().end());
                    if start.line == end.line {
                        remove(start.line - 1, start.column, Some(end.column));
                    } else {
                        remove(start.line - 1, start.column, None);
                        for i in start.line..end.line - 1 {
                            remove(i, 0, None);
                        }
                        remove(end.line - 1, 0, Some(end.column));
                    }
                }
            }
        }
    }

    let doc_span = pulldown_cmark::Parser::new_ext(&doc, pulldown_cmark::Options::all())
        .into_offset_iter()
        .fold(State::None, |mut state, (event, span)| {
            match &state {
                State::None => {
                    if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(
                        pulldown_cmark::CodeBlockKind::Fenced(kind),
                    )) = event
                    {
                        if &*kind == "cargo" {
                            state = State::Start;
                        }
                    }
                }
                State::Start => {
                    if let pulldown_cmark::Event::Text(_) = event {
                        state = State::Text(span);
                    }
                }
                State::Text(span) => {
                    if let pulldown_cmark::Event::End(pulldown_cmark::Tag::CodeBlock(
                        pulldown_cmark::CodeBlockKind::Fenced(kind),
                    )) = event
                    {
                        if &*kind == "cargo" {
                            state = State::End(span.clone());
                        }
                    }
                }
                State::End(_) => {}
            }
            state
        })
        .end()
        .with_context(on_not_found)?;

    let with = if with.is_empty() || with.ends_with('\n') {
        with.to_owned()
    } else {
        format!("{}\n", with)
    };

    let converted_doc = format!("{}{}{}", &doc[..doc_span.start], with, &doc[doc_span.end..]);

    let converted_code = shebang
        .map(Into::into)
        .into_iter()
        .chain(converted_doc.lines().map(|line| {
            if line.is_empty() {
                "//!".into()
            } else {
                format!("//! {}", line).into()
            }
        }))
        .chain(code_lines.into_iter().flatten())
        .interleave_shortest(iter::repeat("\n".into()))
        .join("");

    return Ok((converted_code, doc[doc_span].to_owned()));

    #[derive(Debug)]
    enum State {
        None,
        Start,
        Text(Range<usize>),
        End(Range<usize>),
    }

    impl State {
        fn end(self) -> Option<Range<usize>> {
            match self {
                Self::End(span) => Some(span),
                _ => None,
            }
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
