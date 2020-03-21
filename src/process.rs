use duct::{Expression, IntoExecutablePath};
use itertools::Itertools as _;
use log::info;

use std::ffi::{OsStr, OsString};

pub(crate) fn cmd<T, U>(program: T, args: U) -> Expression
where
    T: IntoExecutablePath,
    U: IntoIterator,
    U::Item: Into<OsString>,
{
    let program = program.to_executable();
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    info(&program, &args, false);
    duct::cmd(program, args)
}

pub(crate) fn run<T, U>(program: T, args: U, dry_run: bool) -> anyhow::Result<()>
where
    T: IntoExecutablePath,
    U: IntoIterator,
    U::Item: Into<OsString>,
{
    let program = program.to_executable();
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    info(&program, &args, false);
    if !dry_run {
        duct::cmd(program, args).run()?;
    }
    Ok(())
}

fn info(program: &OsStr, args: &[OsString], dry_run: bool) {
    info!(
        "{}Running `{}{}`",
        if dry_run { "[dry-run] " } else { "" },
        shell_escape::escape(program.to_string_lossy()),
        args.iter()
            .format_with("", |arg, f| f(&format_args!(" {}", arg.to_string_lossy()))),
    );
}
