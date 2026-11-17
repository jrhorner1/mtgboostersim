//! Logging to the terminal and to a log file.
//!
//! The terminal shows messages at the chosen level (info by default, debug
//! with `--verbose`, or whatever `MTGBOOSTERSIM_LOG` names). The log file,
//! `mtgboostersim.log` in the cache directory, always records debug detail
//! for the latest run.

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const LEVEL_ENV: &str = "MTGBOOSTERSIM_LOG";
const FILE_NAME: &str = "mtgboostersim.log";
const FILE_LEVEL: LevelFilter = LevelFilter::Debug;
/// Libraries (the window toolkit, the graphics stack) are only heard from
/// when something is wrong.
const LIBRARY_LEVEL: LevelFilter = LevelFilter::Warn;

struct Logger {
    terminal: LevelFilter,
    file: Option<Mutex<File>>,
}

impl Logger {
    fn most_verbose(&self) -> LevelFilter {
        if self.file.is_some() { self.terminal.max(FILE_LEVEL) } else { self.terminal }
    }
}

/// Time of day in UTC, to the millisecond.
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}.{:03}Z", secs / 3600, secs / 60 % 60, secs % 60, now.subsec_millis())
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let ours = metadata.target().starts_with(env!("CARGO_CRATE_NAME"));
        metadata.level() <= if ours { self.most_verbose() } else { LIBRARY_LEVEL }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {:<5} {}", timestamp(), record.level(), record.args());
        if record.level() <= self.terminal {
            eprintln!("{line}");
        }
        if let Some(file) = &self.file {
            if record.level() <= FILE_LEVEL {
                let _ = writeln!(file.lock().unwrap(), "{line}");
            }
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            let _ = file.lock().unwrap().flush();
        }
    }
}

fn level_from_env() -> Option<LevelFilter> {
    let value = std::env::var(LEVEL_ENV).ok()?;
    let level = value.parse().ok();
    if level.is_none() {
        eprintln!("{LEVEL_ENV}={value} is not a log level; use off, error, warn, info, debug or trace.");
    }
    level
}

/// Start logging. Returns the log file's path when one could be created.
pub fn init(verbose: bool) -> Option<PathBuf> {
    let terminal = level_from_env()
        .unwrap_or(if verbose { LevelFilter::Debug } else { LevelFilter::Info });
    let path = crate::cache::dir().map(|d| d.join(FILE_NAME));
    let file = path.as_ref().and_then(|p| {
        std::fs::create_dir_all(p.parent()?).ok()?;
        File::create(p).ok()
    });
    let path = path.filter(|_| file.is_some());
    let logger = Logger { terminal, file: file.map(Mutex::new) };
    log::set_max_level(logger.most_verbose().max(LIBRARY_LEVEL));
    if log::set_boxed_logger(Box::new(logger)).is_err() {
        return None;
    }
    if Level::Debug <= terminal {
        log::debug!("Terminal log level: {terminal}");
    }
    path
}
