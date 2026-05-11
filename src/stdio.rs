use std::io::{self, Write};

#[derive(Debug, Clone, Default)]
pub(crate) struct Stdio {
    stdin: Option<Input>,
    pub(crate) stdout: Option<Output>,
    pub(crate) stderr: Option<Output>,
}

impl Stdio {
    pub(crate) fn stdin(&mut self, stdin: Input) -> &mut Self {
        self.stdin = Some(stdin);
        self
    }

    pub(crate) fn stdout(&mut self, stdout: Output) -> &mut Self {
        self.stdout = Some(stdout);
        self
    }

    pub(crate) fn stderr(&mut self, stderr: Output) -> &mut Self {
        self.stderr = Some(stderr);
        self
    }
    
    pub(crate) fn configure(&self, cmd: &mut std::process::Command, default_stdout: Output) {
        if let Some(stdin) = &self.stdin {
            cmd.stdin(stdin.to_stdio());
        }

        let stdout = self.stdout.as_ref().unwrap_or(&default_stdout);
        let stderr = self.stdout.as_ref().unwrap_or(&default_stdout);
        cmd.stdout(stdout.to_stdio());
        cmd.stderr(stderr.to_stdio());
    }

    pub(crate) fn spawn_child(&self, cmd: &mut std::process::Command, default_stdout: Output) -> Result<std::process::Child, SpawnError> {
        self.configure(cmd, default_stdout);
        
        let mut child = cmd.spawn().map_err(SpawnError::Spawn)?;
        if let Some(data) = self.stdin.as_ref().and_then(|it| it.as_data()) {
            child.stdin.take().unwrap().write_all(data).map_err(SpawnError::StdinWrite)?;
        }
        Ok(child)
    }
}

pub(crate) enum SpawnError {
    Spawn(io::Error),
    StdinWrite(io::Error),
}

/// Represents how to setup stdin of a process.
#[derive(Debug, Clone)]
pub struct Input(InputKind);

#[derive(Debug, Clone)]
enum InputKind {
    Null,
    Piped,
    Inherit,
    Data(Vec<u8>),
}

impl From<Vec<u8>> for Input {
    fn from(v: Vec<u8>) -> Self {
        Self(InputKind::Data(v))
    }
}

impl Input {
    /// Provide no input to the process.
    pub fn null() -> Input {
        Self(InputKind::Null)
    }

    /// Pipe input via stream.
    /// 
    /// To use this, you need to run the command via [`read`] method.
    /// 
    /// [`read`]: crate::Cmd::read
    pub fn piped() -> Input {
        Self(InputKind::Piped)
    }

    /// Inherit input from current process.
    pub fn inherit() -> Input {
        Self(InputKind::Inherit)
    }

    /// Provide input data via a buffer to the process. A convenince when you don't want to use pipes youself.
    pub fn data(data: Vec<u8>) -> Input {
        Self(InputKind::Data(data))
    }

    fn as_data(&self) -> Option<&[u8]> {
        match self.0 {
            InputKind::Data(ref data) => Some(data),
            _ => None,
        }
    }

    fn to_stdio(&self) -> std::process::Stdio {
        match self.0 {
            InputKind::Null => std::process::Stdio::null(),
            InputKind::Piped => std::process::Stdio::piped(),
            InputKind::Inherit => std::process::Stdio::inherit(),
            InputKind::Data(_) => std::process::Stdio::piped(),
        }
    }
}

/// Represents how to setup output stream of a process.
/// 
/// The stream can be captured, or non-captured.
/// When the stream is captured corresponding methods collect the stream into a buffer,
/// and allow it to be consumed via methods like [`read`] or collected in the error value.
/// 
/// When stream is captured, it can also be be either collectable, or not.
/// The collectable attribute controls whether if we can collect the output for error reporting.
/// This is generally preferable since it improves error message readability.
/// But in case the output stream is too large, or even infinite, we allow for opt-in to not collect the output.
/// 
/// [`read`]: crate::Cmd::read
#[derive(Debug, Clone)]
pub struct Output(OutputKind);

#[derive(Debug, Clone)]
enum OutputKind {
    Null,
    Piped { collect: bool },
    Inherit,
}

impl Output {
    /// Do not catpure any output.
    pub fn null() -> Output {
        Self(OutputKind::Null)
    }

    /// Capture the output either via buffer, or via the pipe. Captured and collected.
    pub fn piped() -> Output {
        Self(OutputKind::Piped { collect: true })
    }

    /// Capture the output via the pipe. Captured but not collected.
    pub fn piped_stream() -> Output {
        Self(OutputKind::Piped { collect: false })
    }

    /// Inherit current process output streams to child process.
    pub fn inherit() -> Output {
        Self(OutputKind::Inherit)
    }

    pub(crate) fn is_captured(&self) -> bool {
        matches!(self.0, OutputKind::Piped { .. })
    }
    
    pub(crate) fn is_collectable(&self) -> bool {
        matches!(self.0, OutputKind::Piped { collect: true })
    }

    fn to_stdio(&self) -> std::process::Stdio {
        match self.0 {
            OutputKind::Null => std::process::Stdio::null(),
            OutputKind::Piped { .. } => std::process::Stdio::piped(),
            OutputKind::Inherit => std::process::Stdio::inherit(),
        }
    }
}
