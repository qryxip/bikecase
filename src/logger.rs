use crate::AnsiColorChoice;

use log::{Level, LevelFilter, Log, Record};
use once_cell::sync::OnceCell;
use termcolor::{BufferedStandardStream, Color, ColorSpec, WriteColor};

use std::env;
use std::sync::{Arc, Mutex};

pub(crate) fn init_logger(color: AnsiColorChoice) {
    static LOGGER: OnceCell<Logger<BufferedStandardStream>> = OnceCell::new();

    let logger = LOGGER.get_or_init(|| Logger {
        wtr: Arc::new(Mutex::new(BufferedStandardStream::stderr(match color {
            AnsiColorChoice::Auto => {
                if should_enable_for_stderr() {
                    termcolor::ColorChoice::AlwaysAnsi
                } else {
                    termcolor::ColorChoice::Never
                }
            }
            AnsiColorChoice::Always => termcolor::ColorChoice::AlwaysAnsi,
            AnsiColorChoice::Never => termcolor::ColorChoice::Never,
        }))),
    });

    if log::set_logger(logger).is_ok() {
        log::set_max_level(FILTER_LEVEL);
    }
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
            let (header_fg, header) = match record.level() {
                Level::Trace => (Color::Magenta, "trace:"),
                Level::Debug => (Color::Green, "debug:"),
                Level::Info => (Color::Cyan, "info:"),
                Level::Warn => (Color::Yellow, "warn:"),
                Level::Error => (Color::Red, "error:"),
            };

            wtr.set_color(
                ColorSpec::new()
                    .set_fg(Some(header_fg))
                    .set_reset(false)
                    .set_bold(true),
            )
            .unwrap();
            wtr.write_all(header.as_ref()).unwrap();
            wtr.reset().unwrap();
            writeln!(wtr, " {}", record.args()).unwrap();
            wtr.flush().unwrap();
        }
    }

    fn flush(&self) {}
}
