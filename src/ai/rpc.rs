//! Subprocess and JSONL RPC bridge for `pi --mode rpc`.
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

const FOLIO_RPC_EXT_JS: &str = r#"import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export default async function (pi) {
  const rawBase = (process.env.FOLIO_MAGPIE_BASE_URL ?? "").trim().replace(/\/+$/, "");
  if (rawBase) {
    const apiBase = rawBase.endsWith("/v1") ? rawBase : `${rawBase}/v1`;
    const modelsPath = path.join(os.homedir(), ".pi", "agent", "models.json");
    let registered = false;
    try {
      if (fs.existsSync(modelsPath)) {
        const parsed = JSON.parse(fs.readFileSync(modelsPath, "utf8"));
        const magpie = parsed?.providers?.magpie;
        if (magpie && Array.isArray(magpie.models)) {
          if (String(magpie.baseUrl ?? "").replace(/\/+$/, "") !== apiBase) {
            pi.registerProvider("magpie", { ...magpie, baseUrl: apiBase });
          }
          registered = true;
        }
      }
    } catch {}
    if (!registered) {
      try {
        const res = await fetch(`${apiBase}/models`);
        if (res.ok) {
          const payload = await res.json();
          const models = (payload.data ?? []).map((m) => ({
            id: m.id,
            name: m.name ?? m.id,
            reasoning: true,
            input: ["text", "image"],
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
            contextWindow: m.context_window ?? 200000,
            maxTokens: m.max_tokens ?? 32768,
          }));
          if (models.length > 0) {
            pi.registerProvider("magpie", {
              baseUrl: apiBase,
              apiKey: "magpie",
              api: "openai-completions",
              models,
            });
          }
        }
      } catch {}
    }
  }

  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName !== "bash" && event.toolName !== "powershell") return undefined;
    if (!ctx.hasUI) return undefined;
    const command = String(event.input?.command ?? "");
    const ok = await ctx.ui.confirm(`执行 ${event.toolName} 命令`, command);
    if (!ok) {
      return { block: true, reason: "用户拒绝执行命令" };
    }
    return undefined;
  });
}
"#;

#[derive(Debug, Clone)]
pub enum RpcInbound {
    Response {
        command: String,
        success: bool,
        data: Option<Value>,
        error: Option<String>,
    },
    Event(Value),
    ExtensionUiRequest(Value),
    Stderr(String),
    Exited(Option<i32>),
}

pub struct RpcClient {
    stdin_tx: mpsc::Sender<String>,
    pub inbound: mpsc::Receiver<RpcInbound>,
    next_req_id: AtomicU64,
    child: Arc<Mutex<Option<Child>>>,
    #[cfg(windows)]
    _job: Option<ProcessJob>,
}

impl RpcClient {
    pub fn spawn(cwd: &Path, magpie_base_url: &str) -> Result<Self, String> {
        let ext_path = ensure_extension_file()?;
        let launch = resolve_pi_launch()?;

        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.prefix_args);
        cmd.args(["--mode", "rpc", "-e"]);
        cmd.arg(&ext_path);
        cmd.current_dir(cwd);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.env("FOLIO_MAGPIE_BASE_URL", magpie_base_url.trim());
        cmd.env_remove("NO_COLOR");

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("无法启动 pi RPC 进程：{e}"))?;

        #[cfg(windows)]
        let job = ProcessJob::attach(&child).ok();

        let mut stdin = child.stdin.take().ok_or("无法获取 pi stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取 pi stdout")?;
        let stderr = child.stderr.take().ok_or("无法获取 pi stderr")?;

        let (stdin_tx, stdin_rx) = mpsc::channel::<String>();
        let (inbound_tx, inbound_rx) = mpsc::channel::<RpcInbound>();

        // Writer thread
        std::thread::spawn(move || {
            while let Ok(line) = stdin_rx.recv() {
                if stdin.write_all(line.as_bytes()).is_err() {
                    break;
                }
                if !line.ends_with('\n') && stdin.write_all(b"\n").is_err() {
                    break;
                }
                if stdin.flush().is_err() {
                    break;
                }
            }
        });

        // Stdout JSONL reader thread (strict LF framing per pi RPC spec)
        let tx_out = inbound_tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::with_capacity(4096);
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if buf.last() == Some(&b'\n') {
                            buf.pop();
                        }
                        if buf.last() == Some(&b'\r') {
                            buf.pop();
                        }
                        if buf.is_empty() {
                            continue;
                        }
                        let Ok(val) = serde_json::from_slice::<Value>(&buf) else {
                            continue;
                        };
                        let kind = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let msg = match kind {
                            "response" => RpcInbound::Response {
                                command: val
                                    .get("command")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_owned(),
                                success: val
                                    .get("success")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false),
                                data: val.get("data").cloned(),
                                error: val
                                    .get("error")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_owned),
                            },
                            "extension_ui_request" => RpcInbound::ExtensionUiRequest(val),
                            _ => RpcInbound::Event(val),
                        };
                        if tx_out.send(msg).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Stderr diagnostic reader thread
        let tx_err = inbound_tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            let _ = tx_err.send(RpcInbound::Stderr(trimmed.to_owned()));
                        }
                    }
                }
            }
        });

        let child_arc = Arc::new(Mutex::new(Some(child)));
        let child_wait = Arc::clone(&child_arc);
        std::thread::spawn(move || {
            let code = loop {
                let status = {
                    let mut guard = child_wait.lock().expect("child lock");
                    match guard.as_mut() {
                        Some(c) => match c.try_wait() {
                            Ok(Some(st)) => Some(st.code()),
                            Ok(None) => None,
                            Err(_) => Some(None),
                        },
                        None => Some(None),
                    }
                };
                if let Some(code) = status {
                    break code;
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            };
            let _ = inbound_tx.send(RpcInbound::Exited(code));
        });

        Ok(Self {
            stdin_tx,
            inbound: inbound_rx,
            next_req_id: AtomicU64::new(1),
            child: child_arc,
            #[cfg(windows)]
            _job: job,
        })
    }

    pub fn send(&self, mut payload: Value) -> Result<String, String> {
        let id = format!("req-{}", self.next_req_id.fetch_add(1, Ordering::Relaxed));
        if let Some(obj) = payload.as_object_mut() {
            obj.entry("id").or_insert_with(|| Value::String(id.clone()));
        }
        let line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        self.stdin_tx
            .send(line)
            .map_err(|_| "pi RPC 进程输入通道已关闭".to_owned())?;
        Ok(id)
    }

    pub fn send_raw(&self, payload: Value) -> Result<(), String> {
        let line = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        self.stdin_tx
            .send(line)
            .map_err(|_| "pi RPC 进程输入通道已关闭".to_owned())
    }

    pub fn prompt(&self, message: &str) -> Result<String, String> {
        self.send(json!({
            "type": "prompt",
            "message": message,
        }))
    }

    pub fn abort(&self) -> Result<String, String> {
        self.send(json!({ "type": "abort" }))
    }

    pub fn get_state(&self) -> Result<String, String> {
        self.send(json!({ "type": "get_state" }))
    }

    pub fn get_messages(&self) -> Result<String, String> {
        self.send(json!({ "type": "get_messages" }))
    }

    pub fn get_available_models(&self) -> Result<String, String> {
        self.send(json!({ "type": "get_available_models" }))
    }

    pub fn get_available_thinking_levels(&self) -> Result<String, String> {
        self.send(json!({ "type": "get_available_thinking_levels" }))
    }

    pub fn get_session_stats(&self) -> Result<String, String> {
        self.send(json!({ "type": "get_session_stats" }))
    }

    pub fn set_model(&self, provider: &str, model_id: &str) -> Result<String, String> {
        self.send(json!({
            "type": "set_model",
            "provider": provider,
            "modelId": model_id,
        }))
    }

    pub fn set_thinking_level(&self, level: &str) -> Result<String, String> {
        self.send(json!({
            "type": "set_thinking_level",
            "level": level,
        }))
    }

    pub fn new_session(&self) -> Result<String, String> {
        self.send(json!({ "type": "new_session" }))
    }

    pub fn switch_session(&self, path: &str) -> Result<String, String> {
        self.send(json!({
            "type": "switch_session",
            "sessionPath": path,
        }))
    }

    pub fn set_session_name(&self, name: &str) -> Result<String, String> {
        self.send(json!({
            "type": "set_session_name",
            "name": name,
        }))
    }

    pub fn compact(&self) -> Result<String, String> {
        self.send(json!({ "type": "compact" }))
    }

    pub fn get_commands(&self) -> Result<String, String> {
        self.send(json!({ "type": "get_commands" }))
    }

    pub fn clone_session(&self) -> Result<String, String> {
        self.send(json!({ "type": "clone" }))
    }

    pub fn respond_extension_confirm(&self, id: &str, confirmed: bool) -> Result<(), String> {
        self.send_raw(json!({
            "type": "extension_ui_response",
            "id": id,
            "confirmed": confirmed,
        }))
    }

    pub fn respond_extension_value(&self, id: &str, value: &str) -> Result<(), String> {
        self.send_raw(json!({
            "type": "extension_ui_response",
            "id": id,
            "value": value,
        }))
    }

    pub fn respond_extension_cancelled(&self, id: &str) -> Result<(), String> {
        self.send_raw(json!({
            "type": "extension_ui_response",
            "id": id,
            "cancelled": true,
        }))
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
        }
    }
}

struct PiLaunch {
    program: PathBuf,
    prefix_args: Vec<String>,
}

fn ensure_extension_file() -> Result<PathBuf, String> {
    let dir = crate::storage::data_directory()
        .map_err(|e| format!("无法获取数据目录：{e}"))?
        .join("pi");
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建 pi 扩展目录：{e}"))?;
    let path = dir.join("folio-rpc-ext.ts");
    let needs_write = fs::read_to_string(&path).ok().as_deref() != Some(FOLIO_RPC_EXT_JS);
    if needs_write {
        fs::write(&path, FOLIO_RPC_EXT_JS).map_err(|e| format!("写入 pi 扩展失败：{e}"))?;
    }
    Ok(path)
}

fn find_on_path(names: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn resolve_pi_launch() -> Result<PiLaunch, String> {
    let node = find_on_path(&["node.exe", "node"]);
    let candidates = [
        dirs::data_dir().map(|d| {
            d.join("npm")
                .join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("dist")
                .join("bundle")
                .join("cli.js")
        }),
        dirs::data_dir().map(|d| {
            d.join("npm")
                .join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("dist")
                .join("cli.js")
        }),
    ];
    if let Some(node_exe) = &node {
        for candidate in candidates.into_iter().flatten() {
            if candidate.is_file() {
                return Ok(PiLaunch {
                    program: node_exe.clone(),
                    prefix_args: vec![candidate.to_string_lossy().into_owned()],
                });
            }
        }
    }
    if let Some(pi_exe) = find_on_path(&["pi.exe", "pi"]) {
        return Ok(PiLaunch {
            program: pi_exe,
            prefix_args: Vec::new(),
        });
    }
    if let Some(pi_cmd) = find_on_path(&["pi.cmd", "pi.bat"]) {
        if let Some(parent) = pi_cmd.parent() {
            let bundle = parent
                .join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("dist")
                .join("bundle")
                .join("cli.js");
            if bundle.is_file()
                && let Some(node_exe) = node
            {
                return Ok(PiLaunch {
                    program: node_exe,
                    prefix_args: vec![bundle.to_string_lossy().into_owned()],
                });
            }
        }
        return Ok(PiLaunch {
            program: PathBuf::from("cmd.exe"),
            prefix_args: vec!["/C".into(), pi_cmd.to_string_lossy().into_owned()],
        });
    }
    Err("未找到 pi CLI，请确认已全局安装 @earendil-works/pi-coding-agent".into())
}

#[cfg(windows)]
struct ProcessJob(windows::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for ProcessJob {}

#[cfg(windows)]
impl ProcessJob {
    fn attach(child: &Child) -> windows::core::Result<Self> {
        use std::os::windows::io::AsRawHandle;
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
            AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()))?;
            Ok(job)
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
