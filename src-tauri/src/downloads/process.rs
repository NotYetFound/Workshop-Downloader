use std::{
    io::{self, Read, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use ts_rs::TS;
use zeroize::Zeroizing;

use super::{FailureKind, ProviderError};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PromptKind {
    Password,
    GuardCode,
    Mobile,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuthPrompt {
    pub job_id: String,
    pub kind: PromptKind,
}

#[derive(Default)]
struct Authentication {
    prompt: Option<PromptKind>,
    response: Option<Zeroizing<String>>,
}

#[derive(Default)]
pub(super) struct Control {
    cancel: AtomicBool,
    auth: Mutex<Authentication>,
}

impl Control {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    pub fn prompt(&self) -> Option<PromptKind> {
        self.auth.lock().ok()?.prompt
    }

    pub fn set_prompt(&self, kind: Option<PromptKind>) {
        if let Ok(mut auth) = self.auth.lock() {
            auth.prompt = kind;
        }
    }

    pub fn respond(&self, response: Zeroizing<String>) -> bool {
        if self.cancelled() {
            return false;
        }
        let Ok(mut auth) = self.auth.lock() else {
            return false;
        };
        let valid = match auth.prompt {
            Some(PromptKind::Password) => {
                !response.is_empty()
                    && response.len() <= 1024
                    && !response.chars().any(char::is_control)
            }
            Some(PromptKind::GuardCode) => {
                (4..=12).contains(&response.len())
                    && response.bytes().all(|b| b.is_ascii_alphanumeric())
            }
            _ => false,
        };
        if valid && auth.response.is_none() {
            auth.response = Some(response);
            auth.prompt = None;
            return true;
        }
        false
    }

    fn take_response(&self) -> Option<Zeroizing<String>> {
        self.auth.lock().ok()?.response.take()
    }
}

pub(super) fn private_workspace(
    mut command: CommandBuilder,
    workspace: &Path,
    runtime: Option<&Path>,
    dotnet: bool,
) -> std::result::Result<CommandBuilder, ProviderError> {
    let temporary = workspace.join("tmp");
    super::files::ensure_directory(&temporary, true)
        .map_err(|error| ProviderError::new(FailureKind::Files, error.to_string()))?;
    for key in [
        "TMPDIR",
        "TMP",
        "TEMP",
        "DOTNET_BUNDLE_EXTRACT_BASE_DIR",
        "XDG_RUNTIME_DIR",
    ] {
        command.env(key, &temporary);
    }
    command.env_remove("DBUS_SESSION_BUS_ADDRESS");
    command.env("DOTNET_EnableDiagnostics", "0");
    #[cfg(target_os = "linux")]
    {
        let helper = std::env::current_exe().map_err(|error| process_error(error.to_string()))?;
        let mut arguments = vec![helper.into_os_string(), super::sandbox::CHILD_FLAG.into()];
        if dotnet {
            super::sandbox::prepare_dotnet_ipc()
                .map_err(|error| process_error(error.to_string()))?;
            arguments.push("--dotnet-ipc".into());
        }
        arguments.push(workspace.as_os_str().into());
        if let Some(runtime) = runtime {
            arguments.push(runtime.as_os_str().into());
        }
        arguments.push("--".into());
        arguments.append(command.get_argv_mut());
        *command.get_argv_mut() = arguments;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (runtime, dotnet);
    Ok(command)
}

pub(super) fn run(
    command: CommandBuilder,
    control: &Control,
    idle_timeout: Duration,
    mut on_output: impl FnMut(&str, bool),
) -> std::result::Result<portable_pty::ExitStatus, ProviderError> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 32,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| {
            process_error(format!("Could not create the backend terminal. {error}"))
        })?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| process_error(format!("Could not read the backend terminal. {error}")))?;
    let writer = pair.master.take_writer().map_err(|error| {
        process_error(format!("Could not write to the backend terminal. {error}"))
    })?;
    if control.cancelled() {
        return Err(process_error("Download cancelled."));
    }
    let child = pair.slave.spawn_command(command).map_err(|error| {
        ProviderError::new(
            FailureKind::BackendUnavailable,
            format!("Could not start the backend. {error}"),
        )
    })?;
    drop(pair.slave);

    let mut process = Terminal {
        child,
        tree: None,
        writer,
        _master: pair.master,
    };
    process.tree = Some(ProcessTree::new(&*process.child).map_err(|error| {
        process_error(format!("Could not contain the backend process. {error}"))
    })?);
    let (sender, receiver) = mpsc::sync_channel(32);
    let output = thread::Builder::new()
        .name("workshop-cli-output".into())
        .spawn(move || {
            let mut reader = reader;
            let mut buffer = Zeroizing::new([0u8; 4096]);
            while let Ok(count) = reader.read(&mut *buffer) {
                if count == 0
                    || sender
                        .send(Zeroizing::new(buffer[..count].to_vec()))
                        .is_err()
                {
                    break;
                }
            }
        })
        .map_err(|error| process_error(format!("Could not read backend output. {error}")))?;

    let mut decoder = Decoder::default();
    let mut last_activity = Instant::now();
    let mut stream_closed = false;
    let mut exit = None;
    let outcome = loop {
        if control.cancelled() {
            break Err(process_error("Download cancelled."));
        }
        if let Some(response) = control.take_response() {
            if process
                .writer
                .write_all(response.as_bytes())
                .and_then(|_| process.writer.write_all(b"\r"))
                .and_then(|_| process.writer.flush())
                .is_err()
            {
                break Err(process_error(
                    "The backend closed its authentication prompt.",
                ));
            }
            last_activity = Instant::now();
        }

        if !stream_closed {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(bytes) => {
                    decoder.feed(&bytes, &mut on_output);
                    last_activity = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => stream_closed = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        } else {
            thread::sleep(Duration::from_millis(40));
        }
        if exit.is_none() {
            match process.child.try_wait() {
                Ok(Some(status)) => {
                    exit = Some((status, Instant::now()));
                    // The main process exited. Stop descendants that still hold the terminal open.
                    process.stop_tree();
                }
                Ok(None) => {}
                Err(error) => {
                    break Err(process_error(format!(
                        "Could not read the backend exit status. {error}"
                    )));
                }
            }
        }
        if exit
            .as_ref()
            .is_some_and(|(_, time)| stream_closed || time.elapsed() >= Duration::from_secs(2))
        {
            decoder.finish(&mut on_output);
            break Ok(exit.take().expect("exit was checked").0);
        }
        if last_activity.elapsed() >= idle_timeout {
            let kind = if control.prompt().is_some() {
                FailureKind::Authentication
            } else {
                FailureKind::Network
            };
            break Err(ProviderError::new(
                kind,
                "The backend stopped responding. Check your connection or sign-in and retry.",
            ));
        }
    };
    control.set_prompt(None);
    drop(receiver);
    drop(process);
    if output.is_finished() {
        let _ = output.join();
    }
    outcome
}

fn process_error(message: impl Into<String>) -> ProviderError {
    ProviderError::new(FailureKind::Process, message)
}

struct Terminal {
    child: Box<dyn Child + Send + Sync>,
    tree: Option<ProcessTree>,
    writer: Box<dyn Write + Send>,
    _master: Box<dyn MasterPty + Send>,
}

impl Terminal {
    fn stop_tree(&mut self) {
        if let Some(tree) = self.tree.take() {
            tree.stop();
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop_tree();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
struct ProcessTree(rustix::process::Pid);

#[cfg(unix)]
impl ProcessTree {
    fn new(child: &dyn Child) -> io::Result<Self> {
        child
            .process_id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
            .map(Self)
            .ok_or_else(|| io::Error::other("backend PID unavailable"))
    }

    fn stop(self) {
        let _ = rustix::process::kill_process_group(self.0, rustix::process::Signal::KILL);
    }
}

#[cfg(windows)]
struct ProcessTree(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl ProcessTree {
    fn new(child: &dyn Child) -> io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        let process = child
            .as_raw_handle()
            .ok_or_else(|| io::Error::other("backend handle unavailable"))?;
        // The handle is owned by this scope and closes on all failure paths.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // Both handles are live, and the input buffer has the corresponding Win32 layout.
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job.as_raw_handle(), process) == 0
        } {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    fn stop(self) {
        drop(self.0);
    }
}

#[derive(Default)]
enum Escape {
    #[default]
    Text,
    Start,
    Csi,
    Osc,
    OscEnd,
}

#[derive(Default)]
struct Decoder {
    line: Zeroizing<Vec<u8>>,
    escape: Escape,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8], callback: &mut impl FnMut(&str, bool)) {
        for &byte in bytes {
            match (&self.escape, byte) {
                (Escape::Text, b'\x1b') => self.escape = Escape::Start,
                (Escape::Start, b'[') => self.escape = Escape::Csi,
                (Escape::Start, b']') => self.escape = Escape::Osc,
                (Escape::Start, _)
                | (Escape::Csi, 0x40..=0x7e)
                | (Escape::Osc, 7)
                | (Escape::OscEnd, b'\\') => self.escape = Escape::Text,
                (Escape::Osc, b'\x1b') => self.escape = Escape::OscEnd,
                (Escape::OscEnd, _) => self.escape = Escape::Osc,
                (Escape::Text, b'\r' | b'\n') => self.finish(callback),
                (Escape::Text, 8) => {
                    self.line.pop();
                }
                (Escape::Text, byte) if byte >= 32 && self.line.len() < 8192 => {
                    self.line.push(byte)
                }
                _ => {}
            }
        }
        let text = String::from_utf8_lossy(&self.line);
        if text.trim_end().ends_with(':') {
            callback(&text, false);
        }
    }

    fn finish(&mut self, callback: &mut impl FnMut(&str, bool)) {
        if !self.line.is_empty() {
            callback(&String::from_utf8_lossy(&self.line), true);
            // Replace the buffer so zeroization includes terminal echoes.
            self.line = Zeroizing::new(Vec::new());
        }
    }
}
