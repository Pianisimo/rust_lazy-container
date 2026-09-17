//! Runs the `container` CLI and keeps what it reports: exit status, stdout
//! and stderr. Everything here blocks until the command exits, so only the
//! worker threads call it.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::process::{self, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// What a finished `container` command left behind.
#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    code: Option<i32>,
}

impl Output {
    fn new(status: ExitStatus, stdout: &[u8], stderr: &[u8]) -> Self {
        Self {
            success: status.success(),
            stdout: String::from_utf8_lossy(stdout).into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
            code: status.code(),
        }
    }

    /// Stdout when the command succeeded, otherwise what went wrong.
    pub(crate) fn into_result(self) -> Result<String, String> {
        if self.success {
            Ok(self.stdout)
        } else {
            Err(self.failure())
        }
    }

    /// What to tell the user about a failed command: its stderr, else its
    /// stdout (`system status` explains itself there), else the exit code.
    pub(crate) fn failure(&self) -> String {
        [&self.stderr, &self.stdout]
            .into_iter()
            .map(|text| error_message(text.trim()))
            .find(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| match self.code {
                Some(code) => format!("container exited with status {code}"),
                None => "container was stopped by a signal".to_string(),
            })
    }
}

/// Runs `container <args>` and waits for it to exit.
///
/// Stdin is closed: the terminal belongs to the TUI, and a prompt such as the
/// kernel-install question in `system start` must fail rather than wait for
/// an answer nobody can type.
pub(crate) fn run(args: &[&str]) -> io::Result<Output> {
    let output = container(args).output()?;
    Ok(Output::new(output.status, &output.stdout, &output.stderr))
}

/// Like [`run`], for callers that only need stdout or a message to show.
pub(crate) fn output_of(args: &[&str]) -> Result<String, String> {
    run(args)
        .map_err(|e| format!("couldn't run container: {e}"))?
        .into_result()
}

/// Runs a command the user asked for, such as `stop`, and says whether it
/// worked. [`run_detached`] explains how it is run.
pub(crate) fn run_action(args: &[&str]) -> Result<(), String> {
    run_detached(args)
        .map_err(|e| format!("couldn't run container: {e}"))?
        .into_result()
        .map(drop)
}

/// Like [`run_action`], for callers that read the output themselves.
///
/// Unlike [`run`], the command is set up to finish even if the app quits
/// first, since `system stop` stopped halfway leaves the system in pieces:
/// - its output goes to files, not pipes. Once nobody reads a pipe, the
///   CLI's next write to it kills the CLI, and `system stop` logs each step
///   as it goes;
/// - it runs in its own process group, so the hangup sent when the terminal
///   closes doesn't reach it.
pub(crate) fn run_detached(args: &[&str]) -> io::Result<Output> {
    let mut stdout = unlinked_file()?;
    let mut stderr = unlinked_file()?;
    let status = container(args)
        .process_group(0)
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .status()?;
    Ok(Output::new(
        status,
        &read_from_start(&mut stdout)?,
        &read_from_start(&mut stderr)?,
    ))
}

fn container(args: &[&str]) -> Command {
    let mut command = Command::new("container");
    command.args(args).stdin(Stdio::null());
    command
}

/// A temporary file that is deleted as soon as it is open, so nothing is
/// left behind however the app or the command ends.
fn unlinked_file() -> io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("lazy-container-{}-{n}", process::id()));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                fs::remove_file(&path)?;
                return Ok(file);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn read_from_start(file: &mut File) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The part of the CLI's output worth showing. It logs progress lines
/// first and reports the failure itself last, on a line starting with
/// `Error: `; the log lines and that prefix say nothing a failure notice
/// doesn't already say.
fn error_message(text: &str) -> &str {
    let start = text
        .match_indices("Error:")
        .map(|(index, _)| index)
        .find(|&index| index == 0 || text[..index].ends_with('\n'))
        .unwrap_or(0);
    let text = &text[start..];
    text.strip_prefix("Error:")
        .map(str::trim_start)
        .unwrap_or(text)
}

#[cfg(test)]
impl Output {
    /// What a command that exited with `code` would have left behind.
    pub(crate) fn exited(code: i32, stdout: &str, stderr: &str) -> Self {
        Self {
            success: code == 0,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            code: Some(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stdout: &str, stderr: &str, code: Option<i32>) -> Output {
        Output {
            success: false,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            code,
        }
    }

    #[test]
    fn a_failure_reports_stderr_without_the_error_prefix() {
        let output = failed("", "Error: get failed: container web not found\n", Some(1));
        assert_eq!(output.failure(), "get failed: container web not found");
    }

    #[test]
    fn a_failure_keeps_every_line_of_stderr() {
        let output = failed(
            "",
            "Error: Plugin 'container-images' not found.\n\n- If system services are not running, start them with: container system start\n",
            Some(64),
        );
        assert_eq!(
            output.failure(),
            "Plugin 'container-images' not found.\n\n- If system services are not running, start them with: container system start"
        );
    }

    #[test]
    fn a_failure_skips_the_progress_logged_before_the_error() {
        let output = failed(
            "",
            "checking if APIServer is alive\nstopping containers\nError: failed to stop container (cause: \"Error: timeout\")\n",
            Some(1),
        );
        assert_eq!(
            output.failure(),
            "failed to stop container (cause: \"Error: timeout\")"
        );

        // Without an `Error:` line there's nothing to skip to.
        let output = failed("", "stopping containers\nsomething broke\n", Some(1));
        assert_eq!(output.failure(), "stopping containers\nsomething broke");
    }

    #[test]
    fn a_failure_falls_back_to_stdout_then_the_exit_code() {
        assert_eq!(
            failed("apiserver is not running\n", "", Some(1)).failure(),
            "apiserver is not running"
        );
        assert_eq!(
            failed("", "  \n", Some(3)).failure(),
            "container exited with status 3"
        );
        assert_eq!(
            failed("", "", None).failure(),
            "container was stopped by a signal"
        );
    }

    #[test]
    fn success_hands_back_stdout() {
        let output = Output::exited(0, "[]", "warning");
        assert_eq!(output.into_result(), Ok("[]".to_string()));
    }

    #[test]
    fn an_unlinked_file_reads_back_what_was_written() {
        use std::io::Write;

        let mut file = unlinked_file().unwrap();
        let mut writer = file.try_clone().unwrap();
        writer.write_all(b"stopping containers\n").unwrap();
        assert_eq!(
            read_from_start(&mut file).unwrap(),
            b"stopping containers\n"
        );

        // Each call gets a file of its own.
        let mut other = unlinked_file().unwrap();
        assert_eq!(read_from_start(&mut other).unwrap(), b"");
    }
}
