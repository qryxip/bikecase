use crate::ColorChoice;

use log::{info, Level, LevelFilter, Log, Record};
use once_cell::sync::OnceCell;
use termcolor::{BufferedStandardStream, Color, ColorSpec, WriteColor};

use std::fmt::Display;
use std::sync::{Arc, Mutex};
use std::{env, iter};

pub(crate) fn init_logger(color: ColorChoice) {
    static LOGGER: OnceCell<Logger<BufferedStandardStream>> = OnceCell::new();

    let logger = LOGGER.get_or_init(|| Logger {
        wtr: Arc::new(Mutex::new(BufferedStandardStream::stderr(match color {
            ColorChoice::Auto => {
                if should_enable_for_stderr() {
                    termcolor::ColorChoice::AlwaysAnsi
                } else {
                    termcolor::ColorChoice::Never
                }
            }
            ColorChoice::Always => termcolor::ColorChoice::AlwaysAnsi,
            ColorChoice::Never => termcolor::ColorChoice::Never,
        }))),
    });

    if log::set_logger(logger).is_ok() {
        log::set_max_level(FILTER_LEVEL);
    }
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

static FILTER_MODULE: &str = "bikecase";

#[cfg(debug_assertions)]
const FILTER_LEVEL: LevelFilter = LevelFilter::Debug;
#[cfg(not(debug_assertions))]
const FILTER_LEVEL: LevelFilter = LevelFilter::Info;

#[cfg(not(windows))]
fn should_enable_for_stderr() -> bool {
    atty::is(atty::Stream::Stderr) && env::var("TERM").ok().map_or(false, |v| v != "dumb")
}

#[cfg(windows)]
fn should_enable_for_stderr() -> bool {
    use winapi::um::wincon::ENABLE_VIRTUAL_TERMINAL_PROCESSING;
    use winapi_util::HandleRef;

    use std::ops::Deref;

    let term = env::var("TERM");
    let term = term.as_ref().map(Deref::deref);
    if term == Ok("dumb") || term == Ok("cygwin") {
        false
    } else if env::var_os("MSYSTEM").is_some() && term.is_ok() {
        atty::is(atty::Stream::Stderr)
    } else {
        atty::is(atty::Stream::Stderr)
            && winapi_util::console::mode(HandleRef::stderr())
                .ok()
                .map_or(false, |m| m & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0)
    }
}

struct Logger<W> {
    wtr: Arc<Mutex<W>>,
}

impl<W: WriteColor + Sync + Send> Log for Logger<W> {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().split("::").next() == Some(FILTER_MODULE)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut wtr = self.wtr.lock().unwrap();

            let mut write_colored = |s: &str, fg, bold, intense| -> _ {
                wtr.set_color(
                    ColorSpec::new()
                        .set_fg(Some(fg))
                        .set_bold(bold)
                        .set_intense(intense)
                        .set_reset(false),
                )?;
                wtr.write_all(s.as_ref())?;
                wtr.reset()
            };

            write_colored("[", Color::Black, false, true).unwrap();
            match record.level() {
                Level::Trace => write_colored("TRACE", Color::Magenta, true, false),
                Level::Debug => write_colored("DEBUG", Color::Green, true, false),
                Level::Info => write_colored("INFO", Color::Cyan, true, false),
                Level::Warn => write_colored("WARN", Color::Yellow, true, false),
                Level::Error => write_colored("ERROR", Color::Red, true, false),
            }
            .unwrap();
            write_colored("]", Color::Black, false, true).unwrap();
            writeln!(wtr, " {}", record.args()).unwrap();
            wtr.flush().unwrap();
        }
    }

    fn flush(&self) {}
}
