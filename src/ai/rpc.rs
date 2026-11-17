//! Multiplexed JSONL bridge to one SDK host. Dropping a view never kills another session.
use crate::model::AiSettings;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

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
#[derive(Clone, Debug)]
pub struct RpcImage {
    pub data: String,
    pub mime_type: String,
}
struct Route {
    project: String,
    session: Option<String>,
    sender: mpsc::Sender<RpcInbound>,
}

pub struct Host {
    stdin: mpsc::Sender<String>,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    child: Arc<Mutex<Child>>,
    alive: Arc<AtomicBool>,
    next: AtomicU64,
    #[cfg(windows)]
    _job: ProcessJob,
}

pub fn project_key(cwd: &Path) -> String {
    dunce::canonicalize(cwd)
        .expect("workspace directory exists")
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

impl Host {
    pub fn shared(settings: &AiSettings) -> Result<Arc<Self>, String> {
        static SHARED: OnceLock<Mutex<Weak<Host>>> = OnceLock::new();
        let mut shared = SHARED
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .expect("host lock");
        if let Some(host) = shared
            .upgrade()
            .filter(|host| host.alive.load(Ordering::Acquire))
        {
            return Ok(host);
        }
        let packaged = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .expect("executable parent")
            .join("pi-host/host.mjs");
        let script = if packaged.is_file() {
            packaged
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pi-host/host.mjs")
        };
        if !script
            .parent()
            .expect("host directory")
            .join("node_modules")
            .is_dir()
        {
            return Err("SDK 宿主依赖未安装，请运行 scripts/setup-ai.ps1".into());
        }
        let mut command = Command::new("node");
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env(
                "FOLIO_USER_DATA",
                crate::storage::data_directory().map_err(|e| e.to_string())?,
            )
            .env(
                "FOLIO_AI_SETTINGS",
                serde_json::to_string(settings).map_err(|e| e.to_string())?,
            );
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("启动 SDK 宿主失败：{e}"))?;
        #[cfg(windows)]
        let job = ProcessJob::attach(&child).map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in rx {
                if writeln!(stdin, "{line}")
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
        let dispatch = Arc::clone(&routes);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(error) => {
                        for route in dispatch.lock().expect("routes lock").values() {
                            let _ = route
                                .sender
                                .send(RpcInbound::Stderr(format!("宿主协议错误：{error}")));
                        }
                        continue;
                    }
                };
                let message = match value["type"].as_str().unwrap_or("") {
                    "response" => RpcInbound::Response {
                        command: value["command"].as_str().unwrap_or("").into(),
                        success: value["success"].as_bool() == Some(true),
                        data: value.get("data").cloned(),
                        error: value["error"].as_str().map(str::to_owned),
                    },
                    "extension_ui_request" => RpcInbound::ExtensionUiRequest(value.clone()),
                    _ => RpcInbound::Event(value.clone()),
                };
                for (id, route) in dispatch.lock().expect("routes lock").iter() {
                    let addressed = if let Some(target) = value["routeId"].as_str() {
                        target == id
                    } else {
                        value["projectKey"].as_str() == Some(route.project.as_str())
                            && (route.session.is_none()
                                || value["sessionId"].as_str() == route.session.as_deref())
                    };
                    if addressed {
                        let _ = route.sender.send(message.clone());
                    }
                }
            }
        });
        let diagnostic_routes = Arc::clone(&routes);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                for route in diagnostic_routes.lock().expect("routes lock").values() {
                    let _ = route.sender.send(RpcInbound::Stderr(line.clone()));
                }
            }
        });
        let alive = Arc::new(AtomicBool::new(true));
        let child = Arc::new(Mutex::new(child));
        let wait_child = Arc::clone(&child);
        let wait_alive = Arc::clone(&alive);
        let wait_routes = Arc::clone(&routes);
        std::thread::spawn(move || {
            loop {
                let status = wait_child.lock().expect("child lock").try_wait();
                match status {
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    status => {
                        wait_alive.store(false, Ordering::Release);
                        let code = status.ok().flatten().and_then(|status| status.code());
                        for route in wait_routes.lock().expect("routes lock").values() {
                            let _ = route.sender.send(RpcInbound::Exited(code));
                        }
                        break;
                    }
                }
            }
        });
        let host = Arc::new(Self {
            stdin: tx,
            routes,
            child,
            alive,
            next: AtomicU64::new(1),
            #[cfg(windows)]
            _job: job,
        });
        *shared = Arc::downgrade(&host);
        Ok(host)
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.lock().expect("child lock").kill();
    }
}

pub struct RpcClient {
    host: Arc<Host>,
    route: String,
    cwd: PathBuf,
    session: Option<String>,
    pub inbound: mpsc::Receiver<RpcInbound>,
}
impl RpcClient {
    pub fn connect(host: Arc<Host>, cwd: &Path, session: Option<String>) -> Self {
        let route = uuid::Uuid::new_v4().to_string();
        let (sender, inbound) = mpsc::channel();
        host.routes.lock().expect("routes lock").insert(
            route.clone(),
            Route {
                project: project_key(cwd),
                session: session.clone(),
                sender,
            },
        );
        Self {
            host,
            route,
            cwd: cwd.into(),
            session,
            inbound,
        }
    }
    pub fn send(&self, mut payload: Value) -> Result<String, String> {
        let id = format!("req-{}", self.host.next.fetch_add(1, Ordering::Relaxed));
        let obj = payload.as_object_mut().expect("host command object");
        obj.insert("requestId".into(), json!(id));
        obj.insert("routeId".into(), json!(self.route));
        obj.insert("cwd".into(), json!(self.cwd));
        obj.insert("projectKey".into(), json!(project_key(&self.cwd)));
        obj.entry("sessionId")
            .or_insert_with(|| json!(self.session));
        if !self.host.alive.load(Ordering::Acquire) {
            return Err("SDK 宿主已退出，重新打开会话以恢复历史".into());
        }
        self.host
            .stdin
            .send(serde_json::to_string(&payload).map_err(|e| e.to_string())?)
            .map_err(|_| "SDK 宿主输入通道已关闭".to_owned())?;
        Ok(id)
    }
    pub fn send_raw(&self, payload: Value) -> Result<(), String> {
        self.send(payload).map(|_| ())
    }
    pub fn prompt_with_images(
        &self,
        message: &str,
        images: &[RpcImage],
        title: &str,
    ) -> Result<String, String> {
        self.send(json!({ "type": "prompt", "message": message, "titleText": title,
            "images": images.iter().map(|img| json!({ "type": "image", "data": img.data, "mimeType": img.mime_type })).collect::<Vec<_>>() }))
    }
    pub fn abort(&self) -> Result<String, String> {
        self.send(json!({"type":"abort"}))
    }
    pub fn get_state(&self) -> Result<String, String> {
        self.send(json!({"type":"get_state"}))
    }
    pub fn get_messages(&self) -> Result<String, String> {
        self.send(json!({"type":"get_messages"}))
    }
    pub fn get_available_models(&self) -> Result<String, String> {
        self.send(json!({"type":"get_available_models"}))
    }
    pub fn get_available_thinking_levels(&self) -> Result<String, String> {
        self.send(json!({"type":"get_available_thinking_levels"}))
    }
    pub fn get_session_stats(&self) -> Result<String, String> {
        self.send(json!({"type":"get_session_stats"}))
    }
    pub fn get_commands(&self) -> Result<String, String> {
        self.send(json!({"type":"get_commands"}))
    }
    pub fn set_model(&self, provider: &str, model_id: &str) -> Result<String, String> {
        self.send(json!({"type":"set_model", "provider":provider,"modelId":model_id}))
    }
    pub fn set_thinking_level(&self, level: &str) -> Result<String, String> {
        self.send(json!({"type":"set_thinking_level","level":level}))
    }
    pub fn set_session_name(&self, name: &str) -> Result<String, String> {
        self.send(json!({"type":"set_session_name","name":name}))
    }
    pub fn compact(&self) -> Result<String, String> {
        self.send(json!({"type":"compact"}))
    }
    pub fn respond_extension_confirm(&self, id: &str, confirmed: bool) -> Result<(), String> {
        self.send_raw(json!({"type":"extension_ui_response","id":id,"confirmed":confirmed}))
    }
    pub fn respond_extension_value(&self, id: &str, value: &str) -> Result<(), String> {
        self.send_raw(json!({"type":"extension_ui_response","id":id,"value":value}))
    }
    pub fn respond_extension_cancelled(&self, id: &str) -> Result<(), String> {
        self.send_raw(json!({"type":"extension_ui_response","id":id,"cancelled":true}))
    }
}
impl Drop for RpcClient {
    fn drop(&mut self) {
        self.host
            .routes
            .lock()
            .expect("routes lock")
            .remove(&self.route);
    }
}

#[cfg(windows)]
struct ProcessJob(windows::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for ProcessJob {}
#[cfg(windows)]
unsafe impl Sync for ProcessJob {}
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
