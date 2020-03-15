#![warn(rust_2018_idioms)]

mod config;
mod fs;
mod gist;
mod logger;
mod process;
mod rust;
mod workspace;

use crate::config::{BikecaseConfig, BikecaseConfigWorkspace, TildePath};
use crate::gist::PushOptions;
use crate::workspace::{MetadataExt as _, PackageExt as _};

use anyhow::{anyhow, bail, Context as _};
use ignore::WalkBuilder;
use itertools::Itertools as _;
use log::{info, warn};
use structopt::clap::AppSettings;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, IntoStaticStr, VariantNames as _};

use std::convert::TryInto as _;
use std::env;
use std::ffi::OsString;
use std::io::{self, Read as _, Stdout, Write};
use std::path::{Path, PathBuf};

pub fn bikecase<W: Sized, I: FnOnce() -> io::Result<String>, P: Sized>(
    opt: Bikecase,
    ctx: Context<W, I, P>,
) -> anyhow::Result<()> {
    let Bikecase {
        jobs,
        release,
        profile,
        features,
        all_features,
        no_default_features,
        target,
        message_format,
        verbose,
        frozen,
        locked,
        offline,
        bin,
        manifest_path,
        config,
        color,
        file,
        args,
    } = opt;

    let Context {
        cwd,
        home_dir,
        data_local_dir,
        read_input,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let script = file
        .map(|p| crate::fs::read(cwd.join(p.strip_prefix(".").unwrap_or(&p))))
        .unwrap_or_else(|| read_input().map_err(Into::into))?;

    let cargo_toml =
        rust::extract_cargo_lang_code(&script, || "could not find the `cargo` code block")?;

    let config = BikecaseConfig::load_or_create(
        &config,
        home_dir.as_deref(),
        data_local_dir.as_deref(),
        false,
    )?;

    let (workspace_root, manifest_path) = if let Some(manifest_path) = manifest_path {
        let manifest_path = cwd.join(manifest_path.strip_prefix(".").unwrap_or(&manifest_path));
        if !manifest_path.ends_with("Cargo.toml") {
            bail!("the manifest-path must be a path to a Cargo.toml file");
        }
        let workspace_root = manifest_path.parent().expect("should not empty").to_owned();
        (workspace_root, manifest_path)
    } else if let Some(workspace_root) = &config.default {
        let workspace_root = PathBuf::from(workspace_root.expand(home_dir.as_deref()).into_owned());
        let manifest_path = workspace_root.join("Cargo.toml");
        (workspace_root, manifest_path)
    } else {
        bail!(
            "`default` or `--manifest-path` is required: {}",
            config.path().display(),
        );
    };

    if !workspace_root.exists() {
        workspace::create_workspace(workspace_root, false)?;
    }

    let metadata = workspace::cargo_metadata_no_deps(Some(&manifest_path), color, &cwd)?;
    workspace::raise_unless_virtual(&metadata.workspace_root)?;
    let package_name =
        workspace::add_member(&metadata, &cargo_toml, &script, bin.as_deref(), false)?;

    let program = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut program_args = vec![
        "run".into(),
        "-p".into(),
        package_name.into(),
        "--manifest-path".into(),
        manifest_path.into_os_string(),
    ];

    macro_rules! add {
        () => {};
        ($(,)+ $($rest:tt)*) => {
            add!($($rest)*)
        };
        ($var:ident => Flag($opt:literal) $($rest:tt)*) => {
            if $var {
                program_args.push($opt.into());
            }
            add!($($rest)*);
        };
        ($var:ident => Single($opt:literal, $f:expr) $($rest:tt)*) => {
            if let Some(var) = $var {
                program_args.push($opt.into());
                program_args.push(apply($f, var));
            }
            add!($($rest)*);
        };
        ($var:ident => Multiple($opt:literal, $f:expr) $($rest:tt)*) => {
            for var in $var {
                program_args.push($opt.into());
                program_args.push(apply($f, var));
            }
            add!($($rest)*);
        };
        ($var:ident => Occurrences($c:literal) $($rest:tt)*) => {
            if $var > 0 {
                let n = $var.try_into().unwrap_or_else(|_| usize::max_value());
                program_args.push(format!("-{}", itertools::repeat_n($c, n).format("")).into());
            }
            add!($($rest)*);
        }
    }

    add! {
        jobs                => Single("--jobs", |j| j.to_string().into()),
        bin                 => Single("--bin", Into::into),
        release             => Flag("--release"),
        profile             => Single("--profile", Into::into),
        features            => Multiple("--features", Into::into),
        all_features        => Flag("--all-features"),
        no_default_features => Flag("--no-default-features"),
        target              => Single("--target", Into::into),
        message_format      => Multiple("--message-format", Into::into),
        verbose             => Occurrences('v'),
        frozen              => Flag("--frozen"),
        locked              => Flag("--locked"),
        offline             => Flag("--offline"),
    }

    program_args.push("--".into());
    program_args.extend(args);

    crate::process::cmd(program, program_args).run()?;
    return Ok(());

    fn apply<T, F: FnOnce(T) -> OsString>(f: F, arg: T) -> OsString {
        f(arg)
    }
}

pub fn cargo_bikecase<
    W: Write,
    I: FnOnce() -> io::Result<String>,
    P: FnMut(&str) -> io::Result<String>,
>(
    opt: CargoBikecase,
    ctx: Context<W, I, P>,
) -> anyhow::Result<()> {
    match opt {
        CargoBikecase::InitWorkspace(opt) => cargo_bikecase_init_workspace(opt, ctx),
        CargoBikecase::New(opt) => cargo_bikecase_new(opt, ctx),
        CargoBikecase::Rm(opt) => cargo_bikecase_rm(opt, ctx),
        CargoBikecase::Include(opt) => cargo_bikecase_include(opt, ctx),
        CargoBikecase::Exclude(opt) => cargo_bikecase_exclude(opt, ctx),
        CargoBikecase::Import(opt) => cargo_bikecase_import(opt, ctx),
        CargoBikecase::Export(opt) => cargo_bikecase_export(opt, ctx),
        CargoBikecase::Gist(opt) => match opt {
            CargoBikecaseGist::Clone(opt) => cargo_bikecase_gist_clone(opt, ctx),
            CargoBikecaseGist::Pull(opt) => cargo_bikecase_gist_pull(opt, ctx),
            CargoBikecaseGist::Push(opt) => cargo_bikecase_gist_push(opt, ctx),
        },
    }
}

fn cargo_bikecase_init_workspace(
    opt: CargoBikecaseInitWorkspace,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseInitWorkspace {
        color,
        dry_run,
        config,
        path,
    } = opt;

    let Context {
        cwd,
        home_dir,
        data_local_dir,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let path = cwd.join(path.strip_prefix(".").unwrap_or(&path));

    let mut config = BikecaseConfig::load_or_create(
        &config,
        home_dir.as_deref(),
        data_local_dir.as_deref(),
        dry_run,
    )?;

    crate::fs::write(path.join("Cargo.toml"), CARGO_TOML, dry_run)?;

    let program = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let args = vec![
        OsString::from("new"),
        OsString::from("--vcs"),
        OsString::from("none"),
        path.join("template").into(),
    ];
    crate::process::run(program, args, dry_run)?;

    if !dry_run {
        let mut cargo_toml = crate::fs::read_toml_edit(path.join("template").join("Cargo.toml"))?;

        let old_package_version = cargo_toml["package"]["version"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        cargo_toml["package"]["version"] = toml_edit::value("0.0.0");
        info!("`package.version`: {:?} → \"0.0.0\"", old_package_version);
        let old_package_publish = cargo_toml["package"]["publish"].clone();
        cargo_toml["package"]["publish"] = toml_edit::value(false);
        info!("`package.publish`: {:?} → false", old_package_publish);

        crate::fs::write(
            path.join("template").join("Cargo.toml"),
            cargo_toml.to_string(),
            false,
        )?;
    }

    crate::fs::write(
        path.join("template").join("src").join("main.rs"),
        TEMPLATE_SRC_MAIN_RS,
        dry_run,
    )?;

    config
        .workspace_or_default(&path, home_dir.as_deref())?
        .template_package = Some(TildePath::new(
        path.to_str()
            .with_context(|| format!("non UTF-8 path: {:?}", path))?,
        home_dir.as_deref(),
    ));
    return config.save(dry_run);

    static CARGO_TOML: &str = r#"[workspace]
members = ["template"]
exclude = []
"#;

    static TEMPLATE_SRC_MAIN_RS: &str = r#"#!/usr/bin/env bikecase
//! ```cargo
//! # Leave blank.
//! ```

fn main() {
    todo!();
}
"#;
}

fn cargo_bikecase_new(
    opt: CargoBikecaseNew,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseNew {
        manifest_path,
        color,
        name,
        dry_run,
        config,
        path,
    } = opt;

    let Context {
        cwd,
        home_dir,
        data_local_dir,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let cargo_metadata::Metadata { workspace_root, .. } =
        workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;

    let path = cwd.join(path.strip_prefix(".").unwrap_or(&path));

    let config = BikecaseConfig::load_or_create(
        &config,
        home_dir.as_deref(),
        data_local_dir.as_deref(),
        dry_run,
    )?;
    let base = config
        .workspace(&workspace_root, home_dir.as_deref())
        .and_then(
            |BikecaseConfigWorkspace {
                 template_package, ..
             }| template_package.as_ref(),
        )
        .with_context(|| {
            format!(
                "could not find `workspaces.{:?}.template_package`",
                workspace_root,
            )
        })?
        .expand(home_dir.as_deref());
    let base = Path::new(&*base);
    let base = workspace_root.join(base.strip_prefix(".").unwrap_or(base));

    for entry in WalkBuilder::new(&base).hidden(false).build() {
        match entry {
            Ok(entry) => {
                let src = entry.path();
                let dst = path.join(src.strip_prefix(&base)?);
                if !(src.is_dir() || src == base.join("Cargo.toml")) {
                    if let Some(parent) = dst.parent() {
                        if !parent.exists() {
                            crate::fs::create_dir_all(parent, dry_run)?;
                        }
                    }
                    crate::fs::copy(src, &dst, dry_run)?;
                }
            }
            Err(err) => warn!("{}", err),
        }
    }

    let src_manifest_path = base.join("Cargo.toml");
    let mut cargo_toml = crate::fs::read_toml_edit(&src_manifest_path)?;
    let new_package_name = name.as_deref().map(Ok).unwrap_or_else(|| {
        path.file_name()
            .unwrap_or_default()
            .to_str()
            .with_context(|| format!("the file name of `{}` is not valid UTF-8", path.display()))
    })?;
    workspace::modify_package_name(&mut cargo_toml, new_package_name)?;

    let dst_manifest_path = path.join("Cargo.toml");
    crate::fs::write(&dst_manifest_path, cargo_toml.to_string(), dry_run)?;

    workspace::modify_members(
        &workspace_root,
        Some(path.strip_prefix(&base).unwrap_or(&path)),
        None,
        None,
        None,
        dry_run,
    )
}

fn cargo_bikecase_rm(
    opt: CargoBikecaseRm,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseRm {
        manifest_path,
        color,
        dry_run,
        package,
    } = opt;

    let Context {
        cwd, init_logger, ..
    } = ctx;

    init_logger(color);

    let metadata = workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;
    let package = metadata.find_package(&package)?;
    let dir = package
        .manifest_path
        .parent()
        .expect("`manifest_path` should end with \"Cargo.toml\"");

    workspace::modify_members(
        &metadata.workspace_root,
        None,
        None,
        Some(dir),
        Some(dir),
        dry_run,
    )?;

    crate::fs::remove_dir_all(dir, dry_run)
}

fn cargo_bikecase_include(
    opt: CargoBikecaseInclude,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseInclude {
        manifest_path,
        color,
        dry_run,
        path,
    } = opt;

    let Context {
        cwd, init_logger, ..
    } = ctx;

    init_logger(color);

    let cargo_metadata::Metadata { workspace_root, .. } =
        workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;
    let path = cwd.join(path);

    workspace::modify_members(
        &workspace_root,
        Some(&*path),
        None,
        None,
        Some(&*path),
        dry_run,
    )
}

fn cargo_bikecase_exclude(
    opt: CargoBikecaseExclude,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseExclude {
        manifest_path,
        color,
        dry_run,
        path,
    } = opt;

    let Context {
        cwd, init_logger, ..
    } = ctx;

    init_logger(color);

    let cargo_metadata::Metadata { workspace_root, .. } =
        workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;
    let path = cwd.join(path);

    workspace::modify_members(
        &workspace_root,
        None,
        Some(&*path),
        Some(&*path),
        None,
        dry_run,
    )
}

fn cargo_bikecase_import(
    opt: CargoBikecaseImport,
    ctx: Context<impl Sized, impl FnOnce() -> io::Result<String>, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseImport {
        manifest_path,
        color,
        dry_run,
        path,
        file,
    } = opt;

    let Context {
        cwd,
        read_input,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let cargo_metadata::Metadata { workspace_root, .. } =
        workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;

    let content = file
        .as_ref()
        .map(crate::fs::read)
        .unwrap_or_else(move || read_input().map_err(Into::into))?;

    workspace::import_script(&workspace_root, &content, dry_run, |package_name| {
        cwd.join(path.unwrap_or_else(|| workspace_root.join(package_name)))
    })
    .map(drop)
}

fn cargo_bikecase_export(
    opt: CargoBikecaseExport,
    ctx: Context<impl Write, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseExport {
        manifest_path,
        color,
        package,
    } = opt;

    let Context {
        cwd,
        mut stdout,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let metadata = workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;
    let (src_path, cargo_toml) = metadata.find_package(&package)?.find_default_bin()?;
    let (code, _) =
        rust::replace_cargo_lang_code(&crate::fs::read(src_path)?, &cargo_toml, || {
            anyhow!(
                "could not find the `cargo` code block: {}",
                src_path.display(),
            )
        })?;

    stdout.write_all(code.as_ref())?;
    stdout.flush().map_err(Into::into)
}

fn cargo_bikecase_gist_clone(
    opt: CargoBikecaseGistClone,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseGistClone {
        manifest_path,
        color,
        dry_run,
        path,
        config,
        gist_id,
    } = opt;

    let Context {
        cwd,
        home_dir,
        data_local_dir,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let cargo_metadata::Metadata { workspace_root, .. } =
        workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;

    let mut config = BikecaseConfig::load_or_create(
        &config,
        home_dir.as_deref(),
        data_local_dir.as_deref(),
        dry_run,
    )?;
    let gist_ids = &mut config
        .workspace_or_default(&workspace_root, home_dir.as_deref())?
        .gist_ids;

    let (script, _) = gist::retrieve_rust_code(&gist_id)?;
    let package_name =
        workspace::import_script(&workspace_root, &script, dry_run, |package_name| {
            cwd.join(path.unwrap_or_else(|| workspace_root.join(package_name)))
        })?;
    let old_gist_id = gist_ids.get(&package_name).cloned();
    info!(
        "`gist_ids.{:?}`: {:?} -> {:?}",
        package_name, old_gist_id, gist_id,
    );
    gist_ids.insert(package_name, gist_id);
    config.save(dry_run)?;
    Ok(())
}

fn cargo_bikecase_gist_pull(
    opt: CargoBikecaseGistPull,
    ctx: Context<impl Sized, impl Sized, impl Sized>,
) -> anyhow::Result<()> {
    let CargoBikecaseGistPull {
        manifest_path,
        color,
        dry_run,
        config,
        package,
    } = opt;

    let Context {
        cwd,
        home_dir,
        data_local_dir,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let metadata = workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;
    let package = metadata.find_package(&package)?;

    let config = BikecaseConfig::load_or_create(
        &config,
        home_dir.as_deref(),
        data_local_dir.as_deref(),
        dry_run,
    )?;
    let gist_id = config
        .workspace(&metadata.workspace_root, home_dir.as_deref())
        .and_then(|BikecaseConfigWorkspace { gist_ids, .. }| gist_ids.get(&package.name))
        .with_context(|| format!("could not find the `gist_id` for {:?}", package.name))?;

    let (pulled_code, _) = gist::retrieve_rust_code(gist_id)?;
    let (pulled_code, pulled_cargo_toml) =
        rust::replace_cargo_lang_code_with_default(&pulled_code)?;
    let (src_path, prev_cargo_toml) = package.find_default_bin()?;

    for (path, orig, edit) in &[
        (src_path, crate::fs::read(src_path)?, pulled_code),
        (&package.manifest_path, prev_cargo_toml, pulled_cargo_toml),
    ] {
        if orig == edit {
            info!("No changes: {}", path.display());
        } else {
            info!("`{}`:", path.display());
            for diff in diff::lines(orig, edit) {
                let (pref, line) = match diff {
                    diff::Result::Left(l) => ("-", l),
                    diff::Result::Both(l, _) => (" ", l),
                    diff::Result::Right(l) => ("+", l),
                };
                info!("│{}{}", pref, line);
            }
            crate::fs::write(&path, edit, dry_run)?;
        }
    }
    Ok(())
}

fn cargo_bikecase_gist_push(
    opt: CargoBikecaseGistPush,
    ctx: Context<impl Sized, impl Sized, impl FnMut(&str) -> io::Result<String>>,
) -> anyhow::Result<()> {
    let CargoBikecaseGistPush {
        manifest_path,
        color,
        dry_run,
        set_upstream,
        private,
        description,
        config,
        package,
    } = opt;

    let Context {
        cwd,
        home_dir,
        data_local_dir,
        read_password,
        init_logger,
        ..
    } = ctx;

    init_logger(color);

    let metadata = workspace::cargo_metadata_no_deps(manifest_path.as_deref(), color, &cwd)?;

    let mut config = BikecaseConfig::load_or_create(
        &config,
        home_dir.as_deref(),
        data_local_dir.as_deref(),
        dry_run,
    )?;

    let github_token = config
        .github_token
        .as_ref()
        .with_context(|| "missing `github-token`")?
        .load_or_ask(dry_run, home_dir.as_deref(), read_password)?;

    let gist_id = config
        .workspace_or_default(&metadata.workspace_root, home_dir.as_deref())?
        .gist_ids
        .entry(package.clone());

    let (src_path, cargo_toml) = metadata.find_package(&package)?.find_default_bin()?;
    let (code, _) =
        rust::replace_cargo_lang_code(&crate::fs::read(src_path)?, &cargo_toml, || {
            anyhow!(
                "could not find the `cargo` code block: {}",
                src_path.display(),
            )
        })?;

    gist::push(PushOptions {
        github_token: &github_token,
        gist_id,
        code: &code,
        workspace_root: &metadata.workspace_root,
        package: &package,
        set_upstream,
        private,
        description: description.as_deref(),
        dry_run,
    })?;
    config.save(dry_run)
}

#[derive(StructOpt, Debug)]
#[structopt(
    author,
    about,
    settings(&[AppSettings::DeriveDisplayOrder, AppSettings::UnifiedHelpMessage])
)]
pub struct Bikecase {
    /// [cargo] Number of parallel jobs, defaults to # of CPUs
    #[structopt(long, value_name("N"))]
    pub jobs: Option<u32>,

    /// [cargo] Build artifacts in release mode, with optimizations
    #[structopt(long)]
    pub release: bool,

    /// [cargo] Build artifacts with the specified profile
    #[structopt(long, value_name("PROFILE-NAME"))]
    pub profile: Option<String>,

    /// [cargo] Space-separated list of features to activate
    #[structopt(long, value_name("FEATURES"), min_values(1))]
    pub features: Vec<String>,

    /// [cargo] Activate all available features
    #[structopt(long)]
    pub all_features: bool,

    /// [cargo] Do not activate the `default` feature
    #[structopt(long)]
    pub no_default_features: bool,

    /// [cargo] Build for the target triple
    #[structopt(long, value_name("TRIPLE"))]
    pub target: Option<PathBuf>,

    /// [cargo] Error format
    #[structopt(
        long,
        value_name("FMT"),
        case_insensitive(true),
        possible_values(&["human", "json", "short"]),
        default_value("human")
    )]
    pub message_format: Vec<String>,

    /// [cargo] Use verbose output (-vv very verbose/build.rs output)
    #[structopt(short, long, parse(from_occurrences))]
    pub verbose: u32,

    /// [cargo] Require Cargo.lock and cache are up to date
    #[structopt(long)]
    pub frozen: bool,

    /// [cargo] Require Cargo.lock is up to date
    #[structopt(long)]
    pub locked: bool,

    /// [cargo] Run without accessing the network
    #[structopt(long)]
    pub offline: bool,

    /// Save the script as src/bin/<NAME>.rs instead of src/main.rs
    #[structopt(long, value_name("NAME"))]
    pub bin: Option<String>,

    /// Path to the virtual manifest
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// Path to the config file
    #[structopt(long, value_name("PATH"), default_value(&config::PATH))]
    pub config: PathBuf,

    /// Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Path to the script
    pub file: Option<PathBuf>,

    /// Arguments for the compiled program
    #[structopt(parse(from_os_str), raw(true))]
    pub args: Vec<OsString>,
}

#[derive(StructOpt, Debug)]
#[structopt(
    author,
    about,
    global_settings(&[AppSettings::DeriveDisplayOrder, AppSettings::UnifiedHelpMessage])
)]
pub enum Cargo {
    #[structopt(author, about)]
    Bikecase(CargoBikecase),
}

#[derive(StructOpt, Debug)]
pub enum CargoBikecase {
    /// Create a new workspace in an existing directory
    #[structopt(author)]
    InitWorkspace(CargoBikecaseInitWorkspace),

    /// Create a new workspace member from a template
    #[structopt(author)]
    New(CargoBikecaseNew),

    /// Remove a workspace member
    #[structopt(author)]
    Rm(CargoBikecaseRm),

    /// Include a package in the workspace
    #[structopt(author)]
    Include(CargoBikecaseInclude),

    /// Exclude a package from the workspace
    #[structopt(author)]
    Exclude(CargoBikecaseExclude),

    /// Import a script as a package (in the same format as `cargo-script`)
    #[structopt(author)]
    Import(CargoBikecaseImport),

    /// Export a package as a script (in the same format as `cargo-script`)
    #[structopt(author)]
    Export(CargoBikecaseExport),

    /// Gist
    #[structopt(author)]
    Gist(CargoBikecaseGist),
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseInitWorkspace {
    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to the config file
    #[structopt(long, value_name("PATH"), default_value(&config::PATH))]
    pub config: PathBuf,

    /// [cargo] Directory
    #[structopt(default_value("."))]
    pub path: PathBuf,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseNew {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Set the resulting package name, defaults to the directory name
    #[structopt(long, value_name("NAME"))]
    pub name: Option<String>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to the config file
    #[structopt(long, value_name("PATH"), default_value(&config::PATH))]
    pub config: PathBuf,

    /// [cargo] Directory
    pub path: PathBuf,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseRm {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// The **name** of the package to remove
    pub package: String,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseInclude {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to the Cargo package to include
    pub path: String,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseExclude {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to the Cargo package to exclude
    pub path: String,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseImport {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to create the package, defaults to `<workspace-root>/<package-name>`
    #[structopt(long)]
    pub path: Option<PathBuf>,

    /// Path to the script
    pub file: Option<PathBuf>,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseExport {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// The **name** of the package to export
    pub package: String,
}

#[derive(StructOpt, Debug)]
pub enum CargoBikecaseGist {
    /// Clone a script from Gist
    #[structopt(author)]
    Clone(CargoBikecaseGistClone),

    /// Pull a script from Gist
    #[structopt(author)]
    Pull(CargoBikecaseGistPull),

    /// Pull a script to Gist
    #[structopt(author)]
    Push(CargoBikecaseGistPush),
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseGistClone {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to create the package, defaults to `<workspace-root>/<package-name>`
    #[structopt(long)]
    pub path: Option<PathBuf>,

    /// Path to the config file
    #[structopt(long, value_name("PATH"), default_value(&config::PATH))]
    pub config: PathBuf,

    /// Gist ID
    pub gist_id: String,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseGistPull {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Path to the config file
    #[structopt(long, value_name("PATH"), default_value(&config::PATH))]
    pub config: PathBuf,

    /// The **name** of the package to export
    pub package: String,
}

#[derive(StructOpt, Debug)]
pub struct CargoBikecaseGistPush {
    /// [cargo] Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// [cargo] Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(AnsiColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: AnsiColorChoice,

    /// Dry run
    #[structopt(long)]
    pub dry_run: bool,

    /// Create a new gist when `gist_ids.<package>` is not set
    #[structopt(short("u"), long)]
    pub set_upstream: bool,

    /// Make the gist private when `--set-upstream` is enabled
    #[structopt(long)]
    pub private: bool,

    /// Set the description of the gist
    #[structopt(long)]
    pub description: Option<String>,

    /// Path to the config file
    #[structopt(long, value_name("PATH"), default_value(&config::PATH))]
    pub config: PathBuf,

    /// The **name** of the package to push
    pub package: String,
}

#[derive(Debug)]
pub struct Context<W, I, P> {
    pub cwd: PathBuf,
    pub home_dir: Option<PathBuf>,
    pub data_local_dir: Option<PathBuf>,
    pub stdout: W,
    pub read_input: I,
    pub read_password: P,
    pub init_logger: fn(AnsiColorChoice),
}

impl Context<Stdout, fn() -> io::Result<String>, fn(&str) -> io::Result<String>> {
    pub fn new() -> anyhow::Result<Self> {
        use crate::logger::init_logger;

        let cwd = env::current_dir()
            .with_context(|| "couldn't get the current directory of the process")?;
        let home_dir = dirs::home_dir();
        let data_local_dir = dirs::data_local_dir();
        let stdout = io::stdout();

        return Ok(Self {
            cwd,
            home_dir,
            data_local_dir,
            stdout,
            read_input,
            read_password,
            init_logger,
        });

        fn read_input() -> io::Result<String> {
            let mut input = "".to_owned();
            io::stdin().read_to_string(&mut input)?;
            Ok(input)
        }

        fn read_password(prompt: &str) -> io::Result<String> {
            rpassword::read_password_from_tty(Some(prompt))
        }
    }
}

#[derive(EnumString, EnumVariantNames, IntoStaticStr, Debug, Clone, Copy)]
#[strum(serialize_all = "kebab-case")]
pub enum AnsiColorChoice {
    Auto,
    Always,
    Never,
}
