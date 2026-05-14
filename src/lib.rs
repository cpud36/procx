//! This crate provides a simpler and more ergonomic api to construct and run shell commands.
//!
//! There are two main entry points:
//! 1. macro api via [`cmd!`] macro, which allows to create cmd objects as if they were just shell strings,
//! 2. and builder api via [`Cmd`] struct. It's api is similar to the one of [`std::process::Command`], but provides richer api.
//!
//! # Quick start
//!
//! Getting a commit hash of a branch:
//! ```
//! fn commit_hash(branch: &str) -> Result<String, xproc::Error> {
//!     xproc::cmd!("git rev-parse {branch}").read()
//! }
//! ```
//! Here we use the [`cmd!`] macro that parses the string (at compile time) and properly splits it into arguments.
//! It handles escaping by itself, so there is no need to worry about shell-injection attacks (.e.g. branch names with spaces and special characters).
//!
//! The result of the [`cmd!`] macro is a [`Cmd`] struct that has similar api to [`std::process::Command`].
//! Important to note, that [`cmd!`] macro does no io, it only constructs the command object.
//! The exact way to actually run the command is then determined by the method you call on the [`Cmd`].
//!
//! Here is a quick breakdown of the methods on the [`Cmd`], from least to most powerful:
//! 1. [`Cmd::run`] - just runs to completion, expects to exit successfully. IO is inherited.
//! 2. [`Cmd::status`] - same, but allows any exit status.
//! 3. [`Cmd::read`] - runs to (successfull) completion and collects stdout into a string.
//! 4. [`Cmd::read_stderr`] - same, but for stderr.
//! 5. [`Cmd::output`] - runs to completion and captures stdout and stderr. Expects to exit successfully.
//! 6. [`Cmd::output_ignore_status`] - same, but does not expect to exit successfully.
//!
//!
//!
//! You can interpolate a part of the argument:
//! ```
//! # use std::path::Path;
//! fn compile_c_code(input: &Path, output: &Path, opt_level: &str) -> Result<(), xproc::Error> {
//!     xproc::cmd!("cc -O{opt_level} {input} -o {output}").run()
//! }
//! ```
//! Here we interpolate opt_level into the argument getting a command like `cc -O2 hello.c -o hello`.
//!
//! If you want to construct the command step by step you can use more advanced api:
//! ```no_run
//! let input_files = ["hello.c", "world.c"];
//! let output_file = "hello";
//! let debug = false;
//! let opt_level = 2;
//!
//! let output_options = xproc::args!("-o {output_file}");
//! let debug_flag = debug.then_some("-g");
//!
//! let mut cmd = xproc::cmd!("cc {input_files..} {output_options..} {debug_flag..}");
//!
//! if opt_level > 1 {
//!     let opt_level = opt_level.to_string();
//!     cmd.arg(xproc::arg!("-O{opt_level}"));
//! }
//!
//! cmd.run()?;
//! # Ok::<(), xproc::Error>(())
//! ```
//!
//! # See also
//!
//! This crate is inspired by [`xshell`](https://crates.io/crates/xshell),
//! but aims for running larger command lines while keeping the ergonomics of the shell.
//!

use std::{
    ffi::{OsStr, OsString},
    fmt::{self, Debug, Write},
    io::{self, Read},
    path::Path,
    str,
    string::FromUtf8Error,
};

use self::{env::Env, stdio::Stdio};

mod env;
mod stdio;
mod sys;

pub use crate::stdio::{Input, Output};

/// Internal utilities for the macros. Do not use directly.
#[doc(hidden)]
pub mod plumbing {
    use crate::{Args, Cmd};
    use std::ffi::OsString;

    pub use xproc_macros::{arg, args, cmd};

    pub fn new_string() -> OsString {
        OsString::new()
    }

    pub fn new_cmd(cmd: impl Into<OsString>) -> Cmd {
        Cmd::new(cmd.into())
    }

    pub fn new_arg(arg: impl Into<OsString>) -> OsString {
        arg.into()
    }
    pub fn new_args() -> Args {
        Args::new()
    }
}

/// Creates a new command from the given format string
///
/// # Example
///
/// You can write commands with string interpolation and they will be mapped to proper command objects.
/// ```
/// # use xproc::cmd;
/// let output = "out.txt";
/// let cmd = cmd!("magic -o={output}");
/// assert_eq!(cmd.to_string(), r#"`magic -o=out.txt`"#);
/// ```
///
/// Use single quotes to put spaces in the command string. You can put single-quoted strings next to interpolation to merge them.
/// ```
/// # use xproc::cmd;
/// let name = "world";
/// let cmd = cmd!("echo 'single quotes' 'hello '{name}");
/// assert_eq!(cmd.to_string(), r#"`echo "single quotes" "hello world"`"#);
/// ```
///
/// Use `{arg..}` to pass multiple arguments to the command.
/// ```
/// # use xproc::cmd;
/// let multi = ["a", "b", "c"];
/// let optional1 = Some("d");
/// let optional2: Option<&str> = None;
/// let cmd = cmd!("magic {multi..} {optional1..} {optional2..}");
/// assert_eq!(cmd.to_string(), r#"`magic a b c d`"#);
/// ```
/// You cannot merge `{arg..}` with other arguments - map them before passing to the macro.
///
/// N.B. The command must be specified as non-splat argument (e.g. `cmd` or `{cmd}`, but not `{cmd..}`)
///
/// See [`arg!`] and [`args!`] if you want to construct arguments separately.
#[macro_export]
macro_rules! cmd {
    ($cmd:literal) => {{
        // trick r-a to highlight this as a format string
        #[cfg(all(test, not(test)))]
        format_args!($cmd);
        $crate::plumbing::cmd!(($crate::plumbing) $cmd)
    }};
}

/// Creates a new argument from the given format string.
///
/// Syntax is similar to [`cmd!`], but only one argument is allowed.
///
/// ```
/// # use xproc::arg;
/// let arg1 = arg!("hello");
/// assert_eq!(format!("{arg1:?}"), r#""hello""#);
/// let arg2 = arg!("'hello world'");
/// assert_eq!(format!("{arg2:?}"), r#""hello world""#);
/// ```
#[macro_export]
macro_rules! arg {
    ($arg:literal) => {{
        // trick r-a to highlight this as a format string
        #[cfg(all(test, not(test)))]
        format_args!($arg);
        $crate::plumbing::arg!(($crate::plumbing) $arg)
    }};
}

/// Creates a new list of arguments from the given format string.
///
/// Syntax is similar to [`cmd!`], but does not specify the command.
///
/// ```
/// # use xproc::{cmd, args};
/// let multi = ["a", "b", "c"];
/// let output = "out.txt";
/// let args = args!("foo {multi..} -o={output}");
/// let cmd = cmd!("magic {args..}");
/// assert_eq!(cmd.to_string(), r#"`magic foo a b c -o=out.txt`"#);
/// ```
#[macro_export]
macro_rules! args {
    ($arg:literal) => {{
        // trick r-a to highlight this as a format string
        #[cfg(all(test, not(test)))]
        format_args!($arg);
        $crate::plumbing::args!(($crate::plumbing) $arg)
    }};
}

/// A prettier command wrapper that is easier to use in scripts.
#[derive(Debug, Clone)]
pub struct Cmd {
    /// The program to execute.
    program: OsString,
    /// A list of arguments to pass to the program.
    args: Args,
    /// Any environment variables that should be set for the program.
    env: Env,
    /// The directory to run the program from.
    cwd: Option<OsString>,

    stdio: Stdio,
}

impl fmt::Display for Cmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.args.is_empty() {
            write!(f, "`{}`", DisplayArg(&self.program))
        } else {
            write!(
                f,
                "`{} {}`",
                DisplayArg(&self.program),
                self.args.display_content()
            )
        }
    }
}

impl Cmd {
    /// Create a new command with the given program.
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        Cmd {
            program: program.as_ref().to_os_string(),
            args: Args::new(),
            env: Env::default(),
            cwd: None,
            stdio: Stdio::default(),
        }
    }

    /// Add an argument to the command.
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.arg(arg);
        self
    }

    /// Add multiple arguments to the command.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    /// Set an environment variable for the command.
    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.env.set(key.as_ref(), value.as_ref());
        self
    }

    /// Remove an environment variable from the command.
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.env.remove(key.as_ref());
        self
    }

    /// Clear all environment variables from the command.
    pub fn env_clear(&mut self) -> &mut Self {
        self.env.clear();
        self
    }

    /// Set multiple environment variables for the command.
    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, val) in vars {
            self.env(key.as_ref(), val.as_ref());
        }
        self
    }

    /// Set the current working directory for the command.
    pub fn current_dir<P: AsRef<Path>>(&mut self, cwd: P) -> &mut Self {
        self.cwd = Some(cwd.as_ref().as_os_str().to_os_string());
        self
    }

    /// Set the stdin for the command.
    pub fn stdin<T: Into<Input>>(&mut self, stdin: T) -> &mut Self {
        self.stdio.stdin(stdin.into());
        self
    }

    /// Set the stdout for the command.
    pub fn stdout<T: Into<Output>>(&mut self, stdout: T) -> &mut Self {
        self.stdio.stdout(stdout.into());
        self
    }

    pub fn stderr<T: Into<Output>>(&mut self, stderr: T) -> &mut Self {
        self.stdio.stderr(stderr.into());
        self
    }
}

impl Cmd {
    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    pub fn get_args(&self) -> &[OsString] {
        self.args.as_slice()
    }
}

impl Cmd {
    fn base_cmd(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(self.args.iter());
        cmd
    }

    fn build_cmd(&self) -> std::process::Command {
        let mut cmd = self.base_cmd();
        self.env.configure(&mut cmd);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

    /// Spawns the child process and returns a handle to it.
    pub fn spawn(&self) -> Result<std::process::Child, Error> {
        let mut cmd = self.build_cmd();
        let child = self
            .stdio
            .spawn_child(&mut cmd, Output::null())
            .map_err(|error| Error::spawn_error(self, error))?;
        Ok(child)
    }

    /// Runs the process and returns its exit status only.
    ///
    /// If you want to get the output of the process, see [`Self::output`].
    /// If you want to interact with the process, see [`Self::spawn`].
    pub fn status(&self) -> Result<std::process::ExitStatus, Error> {
        let mut cmd = self.build_cmd();
        let mut child = self
            .stdio
            .spawn_child(&mut cmd, Output::null())
            .map_err(|error| Error::spawn_error(self, error))?;
        let status = child
            .wait()
            .map_err(|error| Error::wait_error(self, error))?;
        Ok(status)
    }

    /// Runs the process, waiting for completion, and mapping non-success exit codes to an error.
    pub fn run(&self) -> Result<(), Error> {
        let exit = self.status()?;
        Error::check_exit_status(self, exit)
    }

    /// Runs the process and acquires its output. Only returns ok if the process exited with success.
    ///
    /// By default, stdout and stderr are captured and returned.
    /// If you want to avoid capturing output, configure corresponding stdio to non-captured mode.
    ///
    /// If you want to get the output regardless of the exit status, see [`Self::output_ignore_status`].
    pub fn output(&self) -> Result<std::process::Output, Error> {
        let output = self.output_ignore_status()?;
        Error::check_exit_status(self, output.status)?;
        Ok(output)
    }

    /// Runs the process and aquires its output, regardless of the exit status (success or failure).
    ///
    /// Generally behaves like [`Self::output`], but does not treat non-success exit status as an error.
    pub fn output_ignore_status(&self) -> Result<std::process::Output, Error> {
        let mut cmd = self.build_cmd();
        let mut child = self
            .stdio
            .spawn_child(&mut cmd, Output::piped())
            .map_err(|error| Error::spawn_error(self, error))?;
        drop(child.stdin.take());
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let collect_stdout = self
            .stdio
            .stdout
            .as_ref()
            .is_none_or(|it| it.is_collectable());
        let collect_stderr = self
            .stdio
            .stderr
            .as_ref()
            .is_none_or(|it| it.is_collectable());
        match (
            child.stdout.take().filter(|_| collect_stdout),
            child.stderr.take().filter(|_| collect_stderr),
        ) {
            (None, None) => {}
            (Some(mut out), None) => {
                let res = out.read_to_end(&mut stdout);
                res.unwrap();
            }
            (None, Some(mut err)) => {
                let res = err.read_to_end(&mut stderr);
                res.unwrap();
            }
            (Some(out), Some(err)) => {
                let res = sys::read_output(out, &mut stdout, err, &mut stderr);
                res.unwrap();
            }
        }

        match child.wait() {
            Ok(status) => Ok(std::process::Output {
                status,
                stdout,
                stderr,
            }),
            Err(error) => {
                return Err(Error::wait_error(self, error).output(stdout, stderr));
            }
        }
    }

    /// Reads stdout of the process as a string.
    ///
    /// # Panics
    ///
    /// Requires stdout to be captured.
    pub fn read(&self) -> Result<String, Error> {
        assert!(
            self.stdio.stdout.as_ref().is_none_or(|it| it.is_captured()),
            "cannot call read() if stdout is not captured"
        );
        let output = self.output()?;
        read_stream(output.stdout).map_err(|error| Error::non_utf8_output(self, error))
    }

    /// Reads stderr of the process as a string.
    ///
    /// # Panics
    ///
    /// Requires stderr to be captured.
    pub fn read_stderr(&self) -> Result<String, Error> {
        assert!(
            self.stdio.stderr.as_ref().is_none_or(|it| it.is_captured()),
            "cannot call read_stderr() if stderr is not captured"
        );
        let output = self.output()?;
        read_stream(output.stderr).map_err(|error| Error::non_utf8_output(self, error))
    }
}

fn read_stream(data: Vec<u8>) -> Result<String, FromUtf8Error> {
    let mut stream = String::from_utf8(data)?;
    if stream.ends_with('\n') {
        stream.pop();
    }
    if stream.ends_with('\r') {
        stream.pop();
    }
    Ok(stream)
}

/// A list of arguments. Can be appended to a [`Cmd`].
#[derive(Clone, Default, Debug)]
pub struct Args {
    args: Vec<OsString>,
}

impl std::fmt::Display for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}`", self.display_content())
    }
}

impl Args {
    pub fn new() -> Self {
        Self { args: Vec::new() }
    }
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    fn as_slice(&self) -> &[OsString] {
        &self.args
    }

    fn iter(&self) -> std::slice::Iter<'_, OsString> {
        self.args.iter()
    }

    fn display_content(&self) -> impl std::fmt::Display + '_ {
        struct Display<'a>(&'a [OsString]);
        impl<'a> std::fmt::Display for Display<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut first = true;
                for arg in self.0.iter() {
                    if first {
                        first = false;
                    } else {
                        f.write_char(' ')?;
                    }
                    write!(f, "{}", DisplayArg(arg))?;
                }
                Ok(())
            }
        }
        Display(&self.args)
    }
}

impl IntoIterator for Args {
    type Item = OsString;

    type IntoIter = ArgsIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        ArgsIntoIter {
            args: self.args.into_iter(),
        }
    }
}

pub struct ArgsIntoIter {
    args: std::vec::IntoIter<OsString>,
}

impl Iterator for ArgsIntoIter {
    type Item = OsString;

    fn next(&mut self) -> Option<Self::Item> {
        self.args.next()
    }
}

struct DisplayArg<'a>(&'a OsStr);

impl<'a> fmt::Display for DisplayArg<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn whitelisted(ch: &u8) -> bool {
            matches!(ch, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'=' | b'/' | b',' | b'.' | b'+')
        }

        let bytes = self.0.as_encoded_bytes();
        if bytes.iter().all(whitelisted) && !bytes.is_empty() {
            // just checked we contain only (a subset of) ascii characters
            let s = str::from_utf8(bytes).unwrap();
            f.write_str(s)
        } else {
            fmt::Debug::fmt(self.0, f)
        }
    }
}

#[derive(Debug)]
pub struct Error {
    /// The kind of error that occurred.
    kind: ErrorKind,

    /// The command that failed (if applicable).
    command: String,
    /// The underlying error source (if applicable).
    source: Option<io::Error>,

    /// The exit status of the process.
    ///
    /// This can be `None` if the process failed to launch (like process not
    /// found) or if the exit status wasn't a code but was instead something
    /// like termination via a signal.
    exit_status: Option<std::process::ExitStatus>,

    /// The stdout from the process.
    ///
    /// This can be empty if the process failed to launch, or the output was
    /// not captured.
    stdout: Vec<u8>,
    /// The stderr from the process.
    ///
    /// This can be empty if the process failed to launch, or the output was
    /// not captured.
    stderr: Vec<u8>,
}

impl Error {
    fn new(kind: ErrorKind, cmd: &Cmd, source: Option<io::Error>) -> Self {
        Error {
            kind,
            command: cmd.to_string(),
            source,
            exit_status: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    pub(crate) fn spawn_error(cmd: &Cmd, error: stdio::SpawnError) -> Self {
        match error {
            stdio::SpawnError::Spawn(source) => {
                Self::new(ErrorKind::CouldNotStart, cmd, Some(source))
            }
            stdio::SpawnError::StdinWrite(source) => {
                Self::new(ErrorKind::StdinWriteFailed, cmd, Some(source))
            }
        }
    }

    pub(crate) fn wait_error(cmd: &Cmd, error: io::Error) -> Self {
        Self::new(ErrorKind::WaitFailed, cmd, Some(error))
    }

    pub(crate) fn check_exit_status(cmd: &Cmd, code: std::process::ExitStatus) -> Result<(), Self> {
        if !code.success() {
            Err(Self::execution_error(cmd, code))
        } else {
            Ok(())
        }
    }

    pub(crate) fn execution_error(cmd: &Cmd, code: std::process::ExitStatus) -> Self {
        let mut error = Self::new(ErrorKind::ExecutionFailed, cmd, None);
        error.exit_status = Some(code);
        error
    }

    pub(crate) fn non_utf8_output(cmd: &Cmd, source: FromUtf8Error) -> Self {
        let error = io::Error::new(io::ErrorKind::InvalidData, source);
        Self::new(ErrorKind::NonUtf8Output, cmd, Some(error))
    }

    pub(crate) fn output(mut self, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }

    /// Gets the underlying error source, if available.
    pub fn source(&self) -> Option<&io::Error> {
        self.source.as_ref()
    }

    /// Gets the exit status of the process, if available.
    pub fn exit_status(&self) -> Option<&std::process::ExitStatus> {
        self.exit_status.as_ref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let ErrorKind::Other = self.kind {
            write!(f, "{}", self.source.as_ref().unwrap())?;
            if let Some(code) = &self.exit_status {
                write!(f, " ({code})")?;
            }
        } else {
            write!(f, "{}", self.kind)?;
            if let Some(code) = &self.exit_status {
                write!(f, " ({code})")?;
            }
            if let Some(source) = &self.source {
                write!(f, ": {source}")?;
            }
        }

        write!(f, "\ncommand: {}", self.command)?;

        if let Some(out) = str::from_utf8(&self.stdout)
            .ok()
            .filter(|it| !it.trim().is_empty())
        {
            writeln!(f, "\n--- stdout\n{out}")?;
        }
        if let Some(out) = str::from_utf8(&self.stderr)
            .ok()
            .filter(|it| !it.trim().is_empty())
        {
            writeln!(f, "\n--- stderr\n{out}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source()
            .map(|s| s as &(dyn std::error::Error + 'static))
    }
}

/// The kind of process error that occurred.
#[derive(Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Failed to start the process
    CouldNotStart,
    /// Failed to write stdin data
    StdinWriteFailed,
    /// Process started, but failed to wait
    WaitFailed,
    /// Process started, but return non-zero exit code
    ExecutionFailed,
    /// Process output is not valid UTF-8
    NonUtf8Output,
    Other,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            ErrorKind::CouldNotStart => "could not start the process",
            ErrorKind::StdinWriteFailed => "failed to write stdin data",
            ErrorKind::WaitFailed => "failed to wait for the process to complete",
            ErrorKind::ExecutionFailed => "process exited with error",
            ErrorKind::NonUtf8Output => "process output is not valid UTF-8",
            ErrorKind::Other => "error",
        };
        f.write_str(msg)
    }
}
