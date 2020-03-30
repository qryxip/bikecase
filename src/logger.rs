use log::{info, Level, LevelFilter};

use std::fmt::Display;
use std::io::Write as _;
use std::iter;

pub(crate) fn init(color: crate::ColorChoice) {
    env_logger::Builder::new()
        .format(|buf, record| {
            macro_rules! style(($fg:expr, $intense:expr) => ({
                let mut style = buf.style();
                style.set_color($fg).set_intense($intense);
                style
            }));

            let color = match record.level() {
                Level::Error => env_logger::fmt::Color::Red,
                Level::Warn => env_logger::fmt::Color::Yellow,
                Level::Info => env_logger::fmt::Color::Cyan,
                Level::Debug => env_logger::fmt::Color::Green,
                Level::Trace => env_logger::fmt::Color::White,
            };

            let path = record
                .module_path()
                .map(|p| p.split("::").next().unwrap())
                .filter(|&p| p != module_path!().split("::").next().unwrap())
                .map(|p| format!(" {}", p))
                .unwrap_or_default();

            writeln!(
                buf,
                "{}{}{}{} {}",
                style!(env_logger::fmt::Color::Black, true).value('['),
                style!(color, false).value(record.level()),
                path,
                style!(env_logger::fmt::Color::Black, true).value(']'),
                record.args(),
            )
        })
        .filter_level(LEVEL_FILTER)
        .write_style(color.into())
        .init();

    #[cfg(debug_assertions)]
    const LEVEL_FILTER: LevelFilter = LevelFilter::Debug;
    #[cfg(not(debug_assertions))]
    const LEVEL_FILTER: LevelFilter = LevelFilter::Info;
}

pub(crate) fn info_diff(orig: &str, edit: &str, name: impl Display, str_width: fn(&str) -> usize) {
    let name = name.to_string();

    let max_width = iter::once(&*name)
        .chain(orig.lines())
        .chain(edit.lines())
        .map(str_width)
        .max()
        .unwrap_or(0);

    let horz_bar = (0..max_width / str_width("─") - str_width("┌"))
        .map(|_| '─')
        .collect::<String>();

    info!("┌{}", horz_bar);
    info!("│{}", name);
    info!("├{}", horz_bar);
    for diff in diff::lines(orig, edit) {
        let (pref, line) = match diff {
            diff::Result::Left(l) => ("-", l),
            diff::Result::Both(l, _) => (" ", l),
            diff::Result::Right(l) => ("+", l),
        };
        info!("│{}{}", pref, line);
    }
    info!("└{}", horz_bar);
}
