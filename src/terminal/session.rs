//! One native PTY and shell process tree; its lifetime is independent of visibility.
use crate::model::TerminalSettings;
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
};

pub type PtyMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;

pub struct StartedSession {
    pub process: TerminalProcess,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub shell: String,
}

pub struct TerminalProcess {
    pub master: PtyMaster,
    pub exit: mpsc::Receiver<io::Result<portable_pty::ExitStatus>>,
    // The job is retained until the session is explicitly removed or Folio quits.
    // Killing only the shell PID can otherwise leave an agent child alive.
    job: Option<ProcessJob>,
}

pub fn start(
    settings: &TerminalSettings,
    cwd: &Path,
    is_dark: bool,
) -> Result<StartedSession, String> {
    let shell = if settings.shell.trim().is_empty() {
        find_on_path("pwsh.exe").unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe")
        })
    } else {
        PathBuf::from(settings.shell.trim())
    };
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("无法创建 Windows ConPTY：{error}"))?;
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut command = CommandBuilder::new(&shell);
    // Arguments are already separate values. Loading the normal shell profile
    // preserves user functions such as agy and npm's Codex command entry point.
    command.args(&settings.args);
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "Folio");
    command.env("COLORFGBG", if is_dark { "15;0" } else { "0;15" });
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("无法启动 {}：{error}", shell.display()))?;
    let job = match ProcessJob::attach(child.as_ref()) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            return Err(format!("无法管理终端进程：{error}"));
        }
    };
    // Retaining the slave handle can prevent EOF when the child exits.
    drop(pair.slave);
    let (sender, exit) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(child.wait());
    });
    Ok(StartedSession {
        process: TerminalProcess {
            master: Arc::new(Mutex::new(pair.master)),
            exit,
            job: Some(job),
        },
        reader,
        writer,
        shell: shell
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    })
}

impl Drop for TerminalProcess {
    fn drop(&mut self) {
        // Stop the child tree before closing ConPTY; ClosePseudoConsole may
        // otherwise wait for a live process while its output is being drained.
        self.job.take();
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// A Windows kernel job closes the complete process tree even on normal app exit.
struct ProcessJob(windows::Win32::Foundation::HANDLE);
// Kernel handles may be transferred between the startup worker and UI thread.
unsafe impl Send for ProcessJob {}

impl ProcessJob {
    fn attach(child: &(dyn portable_pty::Child + Send + Sync)) -> windows::core::Result<Self> {
        use windows::Win32::{Foundation::HANDLE, System::JobObjects::*};
        unsafe {
            let job = Self(CreateJobObjectW(None, windows::core::PCWSTR::null())?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                std::mem::size_of_val(&info) as u32,
            )?;
            let raw = child
                .as_raw_handle()
                .expect("ConPTY child has a process handle");
            AssignProcessToJobObject(job.0, HANDLE(raw))?;
            Ok(job)
        }
    }
}

impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
