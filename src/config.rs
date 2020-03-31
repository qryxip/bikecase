use crate::workspace;

use anyhow::{anyhow, Context as _};
use indexmap::{indexmap, IndexMap};
use log::info;
use maplit::btreemap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) static PATH: Lazy<String> = Lazy::new(|| {
    dirs::config_dir()
        .and_then(|d| d.join("bikecase.toml").into_os_string().into_string().ok())
        .unwrap_or_else(|| "bikecase.toml".to_owned())
});

#[derive(Debug)]
pub(crate) struct BikecaseConfig {
    content: BikecaseConfigContent,
    path: PathBuf,
}

impl BikecaseConfig {
    pub(crate) fn load_or_create(
        path: &Path,
        home_dir: Option<&Path>,
        data_local_dir: Option<&Path>,
        dry_run: bool,
    ) -> anyhow::Result<Self> {
        let path = path.to_owned();
        if path.exists() {
            let content = toml::from_str(&crate::fs::read(&path)?)
                .with_context(|| format!("failed to parse the TOML file at {}", path.display()))?;
            Ok(Self { content, path })
        } else {
            let data_local_dir =
                data_local_dir.with_context(|| "could not find the local data directory")?;
            let github_token_path = data_local_dir
                .join("bikecase")
                .join("github-token")
                .into_os_string()
                .into_string()
                .map_err(|s| anyhow!("{:?} is not valid UTF-8", s))?;
            let github_token_path = TildePath::new(&github_token_path, home_dir);
            let default_workspace_expanded = data_local_dir
                .join("bikecase")
                .join("workspace")
                .into_os_string()
                .into_string()
                .map_err(|s| anyhow!("{:?} is not valid UTF-8", s))?;
            let default_workspace = TildePath::new(&default_workspace_expanded, home_dir);
            let template_package_expanded = data_local_dir
                .join("bikecase")
                .join("template")
                .into_os_string()
                .into_string()
                .map_err(|s| anyhow!("{:?} is not valid UTF-8", s))?;
            let template_package = TildePath::new(&template_package_expanded, home_dir);
            let this = Self {
                content: BikecaseConfigContent {
                    github_token: Some(BikecaseConfigGithubToken::File {
                        path: github_token_path,
                    }),
                    default_workspace: Some(default_workspace.clone()),
                    template_package: Some(template_package),
                    workspaces: indexmap!(default_workspace => BikecaseConfigWorkspace {
                        gist_ids: btreemap!(),
                    }),
                },
                path,
            };
            this.save(dry_run)?;
            if !Path::new(&default_workspace_expanded).exists() {
                workspace::create_workspace(default_workspace_expanded, dry_run)?;
            }
            if !Path::new(&template_package_expanded).exists() {
                crate::process::run(
                    workspace::cargo_exe()?,
                    &["new", "--name", "__template", &template_package_expanded],
                    dry_run,
                )?;
                if dry_run {
                    info!("[dry-run] Modifying {}", template_package_expanded);
                } else {
                    info!("Modifying {}", template_package_expanded);
                    let mut cargo_toml = crate::fs::read_toml_edit(
                        Path::new(&template_package_expanded).join("Cargo.toml"),
                    )?;
                    workspace::modify_package_version(&mut cargo_toml, "0.0.0");
                    workspace::modify_package_publish(&mut cargo_toml, false);
                    crate::fs::write(
                        Path::new(&template_package_expanded).join("Cargo.toml"),
                        cargo_toml.to_string(),
                        false,
                    )?;
                    crate::fs::write(
                        Path::new(&template_package_expanded)
                            .join("src")
                            .join("main.rs"),
                        TEMPLATE_PACKAGE_MAIN_RS,
                        false,
                    )?;
                }
            }
            return Ok(this);

            static TEMPLATE_PACKAGE_MAIN_RS: &str = r#"//! ```cargo
//! # Leave blank.
//! ```

fn main() {
    todo!();
}
"#;
        }
    }

    pub(crate) fn save(&self, dry_run: bool) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            crate::fs::create_dir_all(parent, dry_run)?;
        }
        let content = toml::to_string_pretty(&self.content).expect("should not fail");
        crate::fs::write(&self.path, content, dry_run)
    }

    pub(crate) fn content(&self) -> &BikecaseConfigContent {
        &self.content
    }

    pub(crate) fn content_mut(&mut self) -> &mut BikecaseConfigContent {
        &mut self.content
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BikecaseConfigContent {
    #[serde(default)]
    pub(crate) default_workspace: Option<TildePath>,
    #[serde(default)]
    pub(crate) template_package: Option<TildePath>,
    #[serde(default)]
    pub(crate) github_token: Option<BikecaseConfigGithubToken>,
    #[serde(default)]
    pub(crate) workspaces: IndexMap<TildePath, BikecaseConfigWorkspace>,
}

impl BikecaseConfigContent {
    pub(crate) fn workspace(
        &self,
        workspace_root: &Path,
        home_dir: Option<&Path>,
    ) -> Option<&BikecaseConfigWorkspace> {
        self.workspaces
            .iter()
            .find(|(p, _)| Path::new(&*p.expand(home_dir)) == workspace_root)
            .map(|(_, w)| w)
    }

    pub(crate) fn workspace_or_default(
        &mut self,
        workspace_root: &Path,
        home_dir: Option<&Path>,
    ) -> anyhow::Result<&mut BikecaseConfigWorkspace> {
        let key = self
            .workspaces
            .keys()
            .find(|p| Path::new(&*p.expand(home_dir)) == workspace_root)
            .map(|p| Ok::<_, anyhow::Error>(p.clone()))
            .unwrap_or_else(|| {
                let path = workspace_root
                    .to_str()
                    .with_context(|| format!("{:?} is not valid UTF-8 path", workspace_root))?;
                Ok(TildePath::new(path, home_dir))
            })?;

        Ok(self.workspaces.entry(key).or_default())
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "kind")]
pub(crate) enum BikecaseConfigGithubToken {
    File { path: TildePath },
}

impl BikecaseConfigGithubToken {
    pub(crate) fn load_or_ask(
        &self,
        dry_run: bool,
        home_dir: Option<&Path>,
        mut ask: impl FnMut(&str) -> io::Result<String>,
    ) -> anyhow::Result<String> {
        let Self::File { path } = self;
        let path = path.expand(home_dir);
        if Path::new(&*path).exists() {
            crate::fs::read(&*path)
        } else {
            let token = ask("GitHub token: ")?;
            if let Some(parent) = Path::new(&*path).parent() {
                crate::fs::create_dir_all(parent, dry_run)?;
            }
            crate::fs::write(&*path, &token, dry_run)?;
            Ok(token)
        }
    }
}

#[derive(Deserialize, Serialize, Default, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BikecaseConfigWorkspace {
    #[serde(default)]
    pub(crate) gist_ids: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Hash, Clone)]
#[serde(transparent)]
pub(crate) struct TildePath(String);

impl TildePath {
    pub(crate) fn new(path: &str, home_dir: Option<&Path>) -> Self {
        let home_dir = shellexpand::tilde_with_context("~", || home_dir);
        Self(if !path.is_empty() && path.starts_with(&*home_dir) {
            format!("~{}", path.trim_start_matches(&*home_dir))
        } else {
            path.to_owned()
        })
    }

    pub(crate) fn expand(&self, home_dir: Option<&Path>) -> Cow<'_, str> {
        shellexpand::tilde_with_context(&self.0, || home_dir)
    }
}
