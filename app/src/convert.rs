//! Builds the converter's command line and runs it, the way the original program does.

use std::ffi::{OsStr, OsString};
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

/// The log keeps at most this many lines; the oldest are dropped first.
const MAX_LOG_LINES: usize = 5000;

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
/// The input is passed as an OS string, so file names that are not valid Unicode arrive intact.
pub fn build_args(opts: &Options, input: &OsStr) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut flag = |f: &str| args.push(OsString::from(f));
    if opts.dst_to_dsd {
        flag("-c");
    }
    if opts.cue_sheet {
        flag("-C");
    }
    if opts.print {
        flag("-P");
    }
    flag(opts.channels.flag());
    flag(opts.format.flag());
    flag("--input");
    args.push(input.to_os_string());
    args
}

/// One run of the converter.
pub struct Job {
    /// Folder the converter runs in. Its output is written here.
    pub work_dir: PathBuf,
    pub args: Vec<OsString>,
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
    /// Asks for the run to stop: the converter (and anything it started) is killed,
    /// and no later job starts.
    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
        if let Some(child) = lock(&self.child).as_mut() {
            kill_tree(child);
        }
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Kills the process and its children. On Windows, taskkill /T also ends child processes.
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        let _ = hide_console(cmd.args(["/PID", &child.id().to_string(), "/T", "/F"]))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // The converter runs in its own process group (see run_one), so this ends it and its children.
        let _ = Command::new("kill")
            .args(["-s", "KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the background worker sends back to the window.
pub enum Msg {
    Output(String),
    Help(String),
    /// A job is starting: "File index of total: name".
    Start {
        index: usize,
        total: usize,
        name: String,
    },
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
        self.trim();
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
        self.trim();
    }

    /// Drops the oldest lines when the log is too long, so long batches stay responsive.
    fn trim(&mut self) {
        if self.lines.len() > MAX_LOG_LINES {
            let extra = self.lines.len() - MAX_LOG_LINES;
            self.lines.drain(..extra);
        }
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
        let _ = tx.send(Msg::Start {
            index: i + 1,
            total,
            name: job.source.clone(),
        });
        if let Err(e) = run_one(&program_dir, job, &stop, &tx) {
            let name = program_name().trim_start_matches("./");
            let _ = tx.send(Msg::Output(format!(
                "Could not start the converter ({e}). Put {name} in the same folder as this program."
            )));
            break;
        }
    }
    if stop.is_set() {
        let _ = tx.send(Msg::Output(
            "Stopped. Files already written are kept.".into(),
        ));
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
    new_process_group(&mut cmd);
    let mut child = hide_console(&mut cmd).spawn()?;
    let out = child.stdout.take().expect("stdout is piped");
    let err = child.stderr.take().expect("stderr is piped");
    let readers: [JoinHandle<()>; 2] = [pump_thread(out, tx.clone()), pump_thread(err, tx.clone())];
    // A Stop that arrived before the child was stored must still take effect.
    if stop.is_set() {
        kill_tree(&mut child);
    }
    *lock(&stop.child) = Some(child);
    let status = wait_for(stop)?;
    for reader in readers {
        let _ = reader.join();
    }
    if let Some(s) = status {
        if !s.success() && !stop.is_set() {
            let _ = tx.send(Msg::Output(format!("The converter stopped with {s}")));
        }
    }
    report_new_files(&job.work_dir, &before, tx);
    Ok(())
}

/// Waits for the converter to exit, polling so that Stop can end it.
/// Returns None if the process was already taken out of the slot.
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

type Snapshot = Vec<(OsString, (SystemTime, u64))>;

/// Files under `dir`, including subfolders, keyed by their path relative to
/// `dir`, with their modified time and size, sorted by path.
fn snapshot(dir: &Path) -> Snapshot {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Walks `dir` into `files`. Symlinked folders are not followed, so a link
/// loop cannot recurse forever.
fn collect_files(dir: &Path, root: &Path, files: &mut Snapshot) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_files(&entry.path(), root, files);
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| entry.path());
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((rel.into_os_string(), (mtime, meta.len())));
            }
        }
    }
}

/// Lists files that were added or changed in `dir` while the job ran,
/// including files in subfolders.
fn report_new_files(dir: &Path, before: &Snapshot, tx: &Sender<Msg>) {
    let after = snapshot(dir);
    let written: Vec<(String, u64)> = after
        .iter()
        .filter(|entry| before.iter().find(|b| b.0 == entry.0) != Some(entry))
        .map(|(name, (_, len))| (name.to_string_lossy().into_owned(), *len))
        .collect();
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

/// Puts the converter in its own process group, so Stop can end everything it started.
#[cfg(unix)]
fn new_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn new_process_group(_cmd: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_as_strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn default_flags_for_one_file() {
        let args = build_args(&Options::default(), OsStr::new("a.iso"));
        assert_eq!(args_as_strings(&args), ["-2", "-p", "--input", "a.iso"]);
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
        let args = build_args(&opts, OsStr::new("x"));
        assert_eq!(
            args_as_strings(&args),
            ["-c", "-C", "-P", "-m", "-I", "--input", "x"]
        );
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
        assert_eq!(
            log.lines.last().map(String::as_str),
            Some("File 2 of 3: b.iso")
        );
    }

    #[test]
    fn snapshot_lists_files_in_subfolders() {
        let dir = std::env::temp_dir().join(format!("iso2dsd-test-snapshot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("top.dsf"), b"ab").unwrap();
        fs::write(dir.join("sub").join("nested.dsf"), b"abc").unwrap();
        let snap = snapshot(&dir);
        let names: Vec<String> = snap
            .iter()
            .map(|(n, _)| n.to_string_lossy().into_owned())
            .collect();
        let nested = Path::new("sub")
            .join("nested.dsf")
            .to_string_lossy()
            .into_owned();
        assert_eq!(names, [nested, "top.dsf".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_keeps_only_the_newest_lines() {
        let mut log = Log::default();
        for i in 0..(MAX_LOG_LINES + 10) {
            log.push(&format!("line {i}"));
        }
        assert!(log.lines.len() <= MAX_LOG_LINES);
        assert_eq!(
            log.lines.last().map(String::as_str),
            Some(format!("line {}", MAX_LOG_LINES + 9).as_str())
        );
    }
}

/// Runs the real pipeline against a small fake converter (a shell script), so Stop,
/// progress and the output check are tested end to end. Unix only.
#[cfg(all(test, unix))]
mod fake_converter {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Instant;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iso2dsd-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes ./sacd_extract into `dir` with the given shell body.
    fn fake_program(dir: &Path, body: &str) {
        let path = dir.join("sacd_extract");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn job(work_dir: &Path) -> Job {
        Job {
            work_dir: work_dir.to_path_buf(),
            args: vec![OsString::from("--input"), OsString::from("disc.iso")],
            source: "disc.iso".into(),
        }
    }

    /// Everything the worker said, up to and including Done.
    fn drain(rx: Receiver<Msg>) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        let mut done = false;
        for msg in rx.try_iter() {
            match msg {
                Msg::Output(s) => lines.push(s),
                Msg::Done => done = true,
                _ => {}
            }
        }
        (lines, done)
    }

    #[test]
    fn writes_output_and_reports_progress() {
        let program = scratch("ok-program");
        let work = scratch("ok-work");
        fake_program(
            &program,
            "echo 'Processing 1 of 2'; echo 'Processing 2 of 2'; echo data > out.dsf",
        );
        let (tx, rx) = channel();
        run_jobs(program, vec![job(&work)], Stop::default(), tx);
        let (lines, done) = drain(rx);
        assert!(done);
        assert!(
            lines.iter().any(|l| l == "Wrote out.dsf (0.0 MB)"),
            "{lines:?}"
        );
    }

    #[test]
    fn says_so_when_nothing_is_written() {
        let program = scratch("empty-program");
        let work = scratch("empty-work");
        fake_program(&program, "true");
        let (tx, rx) = channel();
        run_jobs(program, vec![job(&work)], Stop::default(), tx);
        let (lines, _) = drain(rx);
        assert!(lines
            .iter()
            .any(|l| l.starts_with("No new files were found")));
    }

    #[test]
    fn stop_ends_a_running_converter_quickly() {
        let program = scratch("stop-program");
        let work = scratch("stop-work");
        fake_program(&program, "sleep 30");
        let (tx, rx) = channel();
        let stop = Stop::default();
        let worker_stop = stop.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || run_jobs(program, vec![job(&work)], worker_stop, tx));
        thread::sleep(Duration::from_millis(300));
        stop.request();
        handle.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        let (lines, done) = drain(rx);
        assert!(done);
        assert!(lines.iter().any(|l| l.starts_with("Stopped.")), "{lines:?}");
    }
}

