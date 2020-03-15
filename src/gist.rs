use anyhow::{anyhow, bail, ensure};
use indexmap::IndexMap;
use itertools::Itertools as _;
use log::info;
use serde::Deserialize;
use serde_json::json;
use ureq::Response;
use url::Url;

use std::collections::btree_map;
use std::path::Path;

pub(crate) fn retrieve_rust_code(gist_id: &str) -> anyhow::Result<(String, String)> {
    let url = "https://api.github.com/gists/"
        .parse::<Url>()
        .unwrap()
        .join(&gist_id)?;

    info!("GET: {}", url);
    let res = ureq::get(url.as_ref()).set("User-Agent", USER_AGENT).call();
    raise_synthetic_error(&res)?;
    info!("{} {}", res.status(), res.status_text());
    ensure!(res.status() == 200, "expected 200");

    let Gist { files, description } = serde_json::from_str(&res.into_string()?)?;

    let file = files
        .values()
        .filter(|GistFile { filename, .. }| {
            [Some("rs".as_ref()), Some("crs".as_ref())].contains(&Path::new(&filename).extension())
        })
        .exactly_one()
        .map_err(|err| {
            let mut err = err.peekable();
            if err.peek().is_some() {
                anyhow!(
                    "multiple Rust files: [{}]",
                    err.format_with(", ", |GistFile { filename, .. }, f| f(&filename)),
                )
            } else {
                anyhow!("no Rust files found")
            }
        })?;

    if file.truncated {
        bail!("{} is truncated", file.filename);
    }

    return Ok((file.content.clone(), description));

    #[derive(Deserialize)]
    struct Gist {
        files: IndexMap<String, GistFile>,
        description: String,
    }

    #[derive(Deserialize, Debug)]
    struct GistFile {
        filename: String,
        truncated: bool,
        content: String,
    }
}

pub(crate) fn push(opts: PushOptions<'_>) -> anyhow::Result<()> {
    let PushOptions {
        github_token,
        mut gist_id,
        code: local,
        workspace_root,
        package,
        set_upstream,
        private,
        description,
        dry_run,
    } = opts;

    let state = if let btree_map::Entry::Occupied(gist_id) = &mut gist_id {
        let gist_id = gist_id.get();
        let (remote_code, remote_description) = retrieve_rust_code(gist_id)?;
        if remote_code == local {
            State::UpToDate
        } else {
            State::Forward(gist_id, remote_description)
        }
    } else {
        State::NotExist
    };

    return match state {
        State::UpToDate => {
            info!("Up to date");
            Ok(())
        }
        State::Forward(gist_id, remote_description) => {
            let url = "https://api.github.com/gists/"
                .parse::<Url>()
                .unwrap()
                .join(gist_id)?;

            if dry_run {
                info!("[dry-run] PATCH {}", url);
            } else {
                let payload = json!({
                    "description": description.unwrap_or(&remote_description),
                    "files": {
                        format!("{}.rs", package): {
                            "content": local
                        }
                    }
                });

                info!("PATCH {}", url);
                let res = ureq::patch(url.as_ref())
                    .set("Authorization", &format!("token {}", github_token))
                    .set("User-Agent", USER_AGENT)
                    .send_json(payload);
                raise_synthetic_error(&res)?;
                info!("{} {}", res.status(), res.status_text());
                ensure!(res.status() == 200, "expected 200");
                serde_json::from_str::<serde_json::Value>(&res.into_string()?)?;

                info!("Updated `{}`", gist_id);
            }
            Ok(())
        }
        State::NotExist => {
            static URL: &str = "https://api.github.com/gists";

            if !set_upstream {
                bail!("to create a new gist, enable `--set-upstream`");
            } else if dry_run {
                info!("[dry-run] POST {}", URL);
                Ok(())
            } else {
                let payload = json!({
                    "files": {
                        format!("{}.rs", package): {
                          "content": local
                        }
                    },
                    "description": description.unwrap_or_default(),
                    "public": !private
                });

                info!("POST {}", URL);
                let res = ureq::post(URL)
                    .set("Authorization", &format!("token {}", github_token))
                    .set("User-Agent", USER_AGENT)
                    .send_json(payload);
                raise_synthetic_error(&res)?;
                ensure!(res.status() == 201, "expected 201");
                let CreateGist { id } = serde_json::from_str(&res.into_string()?)?;
                info!("Created `{}`", id);
                info!(
                    "`workspaces.{:?}.gist_ids.{:?}`: None → Some({:?})",
                    workspace_root, package, id,
                );
                gist_id.or_insert(id);
                Ok(())
            }
        }
    };

    enum State<'a> {
        UpToDate,
        Forward(&'a str, String),
        NotExist,
    }

    #[derive(Deserialize, Debug)]
    struct CreateGist {
        id: String,
    }

    #[derive(Deserialize, Debug)]
    struct Gist {
        files: IndexMap<String, GistFile>,
    }

    #[derive(Deserialize, Debug)]
    struct GistFile {
        filename: String,
        truncated: bool,
        content: String,
    }
}

pub(crate) struct PushOptions<'a> {
    pub(crate) github_token: &'a str,
    pub(crate) gist_id: btree_map::Entry<'a, String, String>,
    pub(crate) code: &'a str,
    pub(crate) workspace_root: &'a Path,
    pub(crate) package: &'a str,
    pub(crate) set_upstream: bool,
    pub(crate) private: bool,
    pub(crate) description: Option<&'a str>,
    pub(crate) dry_run: bool,
}

static USER_AGENT: &str = "cargo-scripts <https://github.com/qryxip/cargo-scripts>";

fn raise_synthetic_error(res: &Response) -> anyhow::Result<()> {
    if let Some(err) = res.synthetic_error() {
        let mut err = err as &dyn std::error::Error;
        let mut displays = vec![err.to_string()];
        while let Some(source) = err.source() {
            displays.push(source.to_string());
            err = source;
        }
        let mut displays = displays.into_iter().rev();
        let cause = anyhow!("{}", displays.next().unwrap());
        return Err(displays.fold(cause, |err, display| err.context(display)));
    }
    Ok(())
}
