//! Builds the converter's command line and runs it, the way the original program does.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Channels {
    #[default]
    Dual,
    Multi,
}

impl Channels {
    pub const ALL: [Channels; 2] = [Channels::Dual, Channels::Multi];

    pub fn label(self) -> &'static str {
        match self {
            Channels::Dual => "Dual",
            Channels::Multi => "Multi",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Channels::Dual => "-2",
            Channels::Multi => "-m",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Format {
    #[default]
    PhilipsDsdiff,
    DsdiffEditMaster,
    SonyDsf,
    RawIso,
}

impl Format {
    pub const ALL: [Format; 4] = [
        Format::PhilipsDsdiff,
        Format::DsdiffEditMaster,
        Format::SonyDsf,
        Format::RawIso,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Format::PhilipsDsdiff => "Philips DSDIFF",
            Format::DsdiffEditMaster => "DSDIFF Edit Master",
            Format::SonyDsf => "Sony DSF",
            Format::RawIso => "Raw ISO",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Format::PhilipsDsdiff => "-p",
            Format::DsdiffEditMaster => "-e",
            Format::SonyDsf => "-s",
            Format::RawIso => "-I",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Options {
    pub channels: Channels,
    pub format: Format,
    pub dst_to_dsd: bool,
    pub cue_sheet: bool,
    pub print: bool,
}

/// Name of the converter program, as the original looks for it.
pub fn program_name() -> &'static str {
    if cfg!(windows) {
        "sacd_extract.exe"
    } else {
        "./sacd_extract"
    }
}

/// Flags in the same order as the original: -c, -C, -P, channels, format, then --input.
pub fn build_args(opts: &Options, input: &str) -> Vec<String> {
    let mut args: Vec<&str> = Vec::new();
    if opts.dst_to_dsd {
        args.push("-c");
    }
    if opts.cue_sheet {
        args.push("-C");
    }
    if opts.print {
        args.push("-P");
    }
    args.push(opts.channels.flag());
    args.push(opts.format.flag());
    args.push("--input");
    args.push(input);
    args.into_iter().map(String::from).collect()
}

/// One run of the converter.
pub struct Job {
    /// Folder the converter runs in. Its output is written here.
    pub work_dir: PathBuf,
    pub args: Vec<String>,
    /// Short name shown in the log, such as the ISO file name.
    pub source: String,
}

/// Lets the window stop a run that is in progress.
#[derive(Clone, Default)]
pub struct Stop {
    flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Stop {
    /// Asks for the run to stop: the converter process is killed and no later job starts.
    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the background worker sends back to the window.
pub enum Msg {
    Output(String),
    Help(String),
    /// A job is starting: "File index of total: name".
    Start { index: usize, total: usize, name: String },
    Done,
}

/// The log shown in the window. Progress lines replace the line before them.
#[derive(Default)]
pub struct Log {
    pub lines: Vec<String>,
    /// Progress of the file being converted, 0.0 to 1.0, when the converter reports it.
    pub progress: Option<f32>,
    last_was_progress: bool,
}

impl Log {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.progress = None;
        self.last_was_progress = false;
    }

    pub fn start_file(&mut self, index: usize, total: usize, name: &str) {
        if !self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.lines.push(format!("File {index} of {total}: {name}"));
        self.progress = None;
        self.last_was_progress = false;
    }

    pub fn push(&mut self, raw: &str) {
        let text = raw.replace("\u{1b}[2K", "").replace("[2K", "");
        let value = parse_progress(&text);
        let progress = value.is_some();
        if let Some(v) = value {
            self.progress = Some(v);
        }
        if progress && self.last_was_progress && !self.lines.is_empty() {
            *self.lines.last_mut().expect("checked non-empty") = text;
        } else if text.contains("We are done") {
            self.lines.push(format!("\n{text}"));
        } else {
            self.lines.push(text);
        }
        self.last_was_progress = progress;
    }
}

/// Reads a progress line such as "Processing 2 of 3" (2/3) or "Completed: 45%" (0.45).
/// The exact wording of the converter's lines is assumed, not confirmed.
fn parse_progress(text: &str) -> Option<f32> {
    if !(text.contains("Completed:") || text.contains("Processing")) {
        return None;
    }
    let numbers: Vec<f64> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    let value = if text.contains('%') {
        *numbers.first()? / 100.0
    } else {
        let (done, total) = (*numbers.first()?, *numbers.get(1)?);
        if total <= 0.0 {
            return None;
        }
        done / total
    };
    Some(value.clamp(0.0, 1.0) as f32)
}

/// Runs the converter once per job, one after another, then reports Done.
pub fn run_jobs(program_dir: PathBuf, jobs: Vec<Job>, stop: Stop, tx: Sender<Msg>) {
    let total = jobs.len();
    for (i, job) in jobs.iter().enumerate() {
        if stop.is_set() {
            break;
        }
        let _ = tx.send(Msg::Start { index: i + 1, total, name: job.source.clone() });
        if let Err(e) = run_one(&program_dir, job, &stop, &tx) {
            let name = program_name().trim_start_matches("./");
            let _ = tx.send(Msg::Output(format!(
                "Could not start the converter ({e}). Put {name} in the same folder as this program."
            )));
            break;
        }
    }
    if stop.is_set() {
        let _ = tx.send(Msg::Output("Stopped. Files already written are kept.".into()));
    }
    let _ = tx.send(Msg::Done);
}

/// Runs the converter with -? and sends its help text.
pub fn run_help(dir: PathBuf, tx: Sender<Msg>) {
    let mut cmd = Command::new(dir.join(program_name()));
    let text = match hide_console(cmd.arg("-?").current_dir(&dir).stdin(Stdio::null())).output() {
        Ok(out) => format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(e) => format!("Could not start the converter: {e}"),
    };
    let _ = tx.send(Msg::Help(text));
    let _ = tx.send(Msg::Done);
}

fn run_one(program_dir: &Path, job: &Job, stop: &Stop, tx: &Sender<Msg>) -> io::Result<()> {
    let before = snapshot(&job.work_dir);
    let mut cmd = Command::new(program_dir.join(program_name()));
    cmd.args(&job.args)
        .current_dir(&job.work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = hide_console(&mut cmd).spawn()?;
    let out = child.stdout.take().expect("stdout is piped");
    let err = child.stderr.take().expect("stderr is piped");
    let readers: [JoinHandle<()>; 2] = [pump_thread(out, tx.clone()), pump_thread(err, tx.clone())];
    if stop.is_set() {
        let _ = child.kill();
    }
    *lock(&stop.child) = Some(child);
    let status = wait_for(stop)?;
    for reader in readers {
        let _ = reader.join();
    }
    match status {
        Some(s) if !s.success() && !stop.is_set() => {
            let _ = tx.send(Msg::Output(format!("The converter stopped with {s}")));
        }
        _ => {}
    }
    report_new_files(&job.work_dir, &before, tx);
    Ok(())
}

/// Waits for the converter to exit, polling so that Stop can end it. None if it was already stopped.
fn wait_for(stop: &Stop) -> io::Result<Option<ExitStatus>> {
    loop {
        {
            let mut slot = lock(&stop.child);
            let polled = match slot.as_mut() {
                None => return Ok(None),
                Some(child) => child.try_wait()?,
            };
            if let Some(status) = polled {
                *slot = None;
                return Ok(Some(status));
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

type Snapshot = HashMap<OsString, (SystemTime, u64)>;

/// Files directly in `dir`, with their modified time and size.
fn snapshot(dir: &Path) -> Snapshot {
    let mut map = HashMap::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    map.insert(entry.file_name(), (mtime, meta.len()));
                }
            }
        }
    }
    map
}

/// Lists files that were added or changed in `dir` while the job ran.
/// Only files directly in that folder are checked; files in subfolders are not listed.
fn report_new_files(dir: &Path, before: &Snapshot, tx: &Sender<Msg>) {
    let after = snapshot(dir);
    let mut written: Vec<(String, u64)> = after
        .iter()
        .filter(|(name, v)| before.get(*name) != Some(*v))
        .map(|(name, (_, len))| (name.to_string_lossy().into_owned(), *len))
        .collect();
    written.sort();
    if written.is_empty() {
        let _ = tx.send(Msg::Output(format!(
            "No new files were found in {}. Check the output above.",
            dir.display()
        )));
        return;
    }
    for (name, len) in written {
        let _ = tx.send(Msg::Output(format!(
            "Wrote {name} ({:.1} MB)",
            len as f64 / 1_048_576.0
        )));
    }
}

fn pump_thread<R: Read + Send + 'static>(reader: R, tx: Sender<Msg>) -> JoinHandle<()> {
    thread::spawn(move || pump(reader, &tx))
}

/// Splits output into lines on \n, \r or \r\n, as Java's readLine does.
fn pump<R: Read>(reader: R, tx: &Sender<Msg>) {
    let mut line: Vec<u8> = Vec::new();
    let mut after_cr = false;
    for byte in BufReader::new(reader).bytes() {
        let Ok(b) = byte else { break };
        match b {
            b'\n' if after_cr => after_cr = false,
            b'\r' | b'\n' => {
                after_cr = b == b'\r';
                let _ = tx.send(Msg::Output(String::from_utf8_lossy(&line).into_owned()));
                line.clear();
            }
            _ => {
                after_cr = false;
                line.push(b);
            }
        }
    }
    if !line.is_empty() {
        let _ = tx.send(Msg::Output(String::from_utf8_lossy(&line).into_owned()));
    }
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: don't open a console window for the converter.
    cmd.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
fn hide_console(cmd: &mut Command) -> &mut Command {
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_flags_for_one_file() {
        let args = build_args(&Options::default(), "a.iso");
        assert_eq!(args, ["-2", "-p", "--input", "a.iso"]);
    }

    #[test]
    fn all_flags_keep_the_original_order() {
        let opts = Options {
            channels: Channels::Multi,
            format: Format::RawIso,
            dst_to_dsd: true,
            cue_sheet: true,
            print: true,
        };
        assert_eq!(build_args(&opts, "x"), ["-c", "-C", "-P", "-m", "-I", "--input", "x"]);
    }

    #[test]
    fn progress_line_replaces_only_a_progress_line() {
        let mut log = Log::default();
        log.push("Reading disc");
        log.push("Processing 1 of 3");
        log.push("Processing 2 of 3");
        assert_eq!(log.lines, ["Reading disc", "Processing 2 of 3"]);
        log.push("We are done");
        assert_eq!(log.lines.last().map(String::as_str), Some("\nWe are done"));
    }

    #[test]
    fn progress_is_read_from_count_and_percent() {
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
        assert!(close(parse_progress("Processing 1 of 4"), 0.25));
        assert!(close(parse_progress("Completed: 45%"), 0.45));
        assert!(parse_progress("Reading disc").is_none());
        assert!(parse_progress("Processing 0 of 0").is_none());
    }

    #[test]
    fn start_file_adds_a_header_and_resets_progress() {
        let mut log = Log::default();
        log.push("Processing 1 of 2");
        assert!(log.progress.is_some());
        log.start_file(2, 3, "b.iso");
        assert_eq!(log.progress, None);
        assert_eq!(log.lines.last().map(String::as_str), Some("File 2 of 3: b.iso"));
    }
}
