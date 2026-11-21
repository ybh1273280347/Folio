//! Read and write native Draw.io files for the AI workspace tools.
use super::Workspace;
use super::documents::PaneKey;
use crate::{ai::WorkspaceToolRequest, files};
use gpui_kit::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug)]
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn result(self) -> Value {
        json!({ "status": "error", "error": { "code": self.code, "message": self.message } })
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Read,
    Create,
    Update,
    GetSelection,
    EditCells,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    path: Option<String>,
    xml: Option<String>,
    operations: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: Operation,
    params: Params,
}

impl Workspace {
    pub(super) fn drawio_request(
        &mut self,
        request: &WorkspaceToolRequest,
        cx: &mut Context<Self>,
    ) {
        if request.action == crate::ai::WorkspaceToolAction::Cancel {
            self.drawio_requests.remove(&request.id);
            return;
        }
        let operation: Request = match serde_json::from_value(request.arguments.clone()) {
            Ok(operation) => operation,
            Err(error) => {
                self.ai.read(cx).respond_workspace_tool(
                    request,
                    Failure::new("invalid_request", error.to_string()).result(),
                );
                return;
            }
        };
        if matches!(operation.operation, Operation::GetSelection | Operation::EditCells) {
            self.drawio_canvas_request(request, operation, cx);
            return;
        }
        let result = self.drawio_result(request);
        self.ai.read(cx).respond_workspace_tool(request, result);
    }

    fn drawio_canvas_request(
        &mut self,
        request: &WorkspaceToolRequest,
        operation: Request,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.directory_workspace() else {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("project_not_active", "Draw.io 工具需要当前项目或默认空间。").result(),
            );
            return;
        };
        let root = match dunce::canonicalize(&project.folder) {
            Ok(root) => root,
            Err(error) => {
                self.ai.read(cx).respond_workspace_tool(
                    request,
                    Failure::new("project_unavailable", error.to_string()).result(),
                );
                return;
            }
        };
        if request.project_key != crate::ai::project_key(&root) {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("project_not_active", "AI 会话所属项目已切换，请在当前项目重试。").result(),
            );
            return;
        }
        let path = self.documents.active.clone();
        if path.is_empty() || !files::is_drawio(Path::new(&path)) {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("drawio_not_active", "请先打开要操作的 Draw.io 图表并切换到该页签。").result(),
            );
            return;
        }
        let Some(pane) = self.documents.drawio_panes.get(&PaneKey::tab(&path)).cloned() else {
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new("drawio_not_ready", "当前 Draw.io 画布尚未就绪，请稍后重试。").result(),
            );
            return;
        };
        let operation_name = match operation.operation {
            Operation::GetSelection => "get_selection",
            Operation::EditCells => "edit_cells",
            _ => unreachable!(),
        };
        let params = serde_json::to_value(operation.params).expect("Draw.io parameters serialize");
        self.drawio_requests.insert(request.id.clone(), request.clone());
        let dispatch = pane.update(cx, |pane, cx| {
            pane.request_tool(&request.id, operation_name, params, cx)
        });
        if let Err(code) = dispatch {
            self.drawio_requests.remove(&request.id);
            self.ai.read(cx).respond_workspace_tool(
                request,
                Failure::new(code, match code {
                    "drawio_bridge_unavailable" => "Folio Draw.io 工具桥尚未就绪，请稍后重试。",
                    "drawio_dispatch_failed" => "Folio 无法向当前 Draw.io 画布派发工具请求。",
                    _ => "当前 Draw.io 画布尚未就绪，请稍后重试。",
                }).result(),
            );
        }
    }

    pub(super) fn drawio_tool_result(
        &mut self,
        request_id: &str,
        result: Value,
        cx: &mut Context<Self>,
    ) {
        if let Some(request) = self.drawio_requests.remove(request_id) {
            self.ai.read(cx).respond_workspace_tool(&request, result);
        }
    }

    fn drawio_result(&self, request: &WorkspaceToolRequest) -> Value {
        let operation: Request = match serde_json::from_value(request.arguments.clone()) {
            Ok(operation) => operation,
            Err(error) => return Failure::new("invalid_request", error.to_string()).result(),
        };
        let Some(project) = self.directory_workspace() else {
            return Failure::new("project_not_active", "Draw.io 工具需要当前项目或默认空间。")
                .result();
        };
        let root = match dunce::canonicalize(&project.folder) {
            Ok(root) => root,
            Err(error) => return Failure::new("project_unavailable", error.to_string()).result(),
        };
        if request.project_key != crate::ai::project_key(&root) {
            return Failure::new(
                "project_not_active",
                "AI 会话所属项目已切换，请在当前项目重试。",
            )
            .result();
        }
        if operation.params.path.is_none() {
            return Failure::new("invalid_request", "文件操作必须提供 path。").result();
        }
        execute(&root, operation).unwrap_or_else(Failure::result)
    }
}

fn execute(root: &Path, request: Request) -> Result<Value, Failure> {
    let relative = validate_relative_path(request.params.path.as_deref().unwrap_or_default())?;
    let path_text = relative.to_string_lossy().replace('\\', "/");
    match request.operation {
        Operation::Read => {
            let path = existing_path(root, &relative)?;
            let xml = fs::read_to_string(&path)
                .map_err(|error| Failure::new("read_failed", error.to_string()))?;
            Ok(json!({ "status": "ok", "path": path_text, "xml": xml }))
        }
        Operation::Create => {
            let xml = request
                .params
                .xml
                .ok_or_else(|| Failure::new("invalid_request", "创建图表时必须提供 xml。"))?;
            validate_xml(&xml)?;
            let path = new_path(root, &relative)?;
            // create_new makes the no-overwrite rule atomic with the file creation.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| {
                    let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
                        "file_exists"
                    } else {
                        "write_failed"
                    };
                    Failure::new(code, error.to_string())
                })?;
            file.write_all(xml.as_bytes())
                .map_err(|error| Failure::new("write_failed", error.to_string()))?;
            Ok(json!({ "status": "ok", "path": path_text }))
        }
        Operation::Update => {
            let xml = request
                .params
                .xml
                .ok_or_else(|| Failure::new("invalid_request", "更新图表时必须提供 xml。"))?;
            validate_xml(&xml)?;
            let path = existing_path(root, &relative)?;
            fs::write(path, xml.as_bytes())
                .map_err(|error| Failure::new("write_failed", error.to_string()))?;
            Ok(json!({ "status": "ok", "path": path_text }))
        }
        Operation::GetSelection | Operation::EditCells => {
            unreachable!("canvas operations are dispatched to the active Draw.io pane")
        }
    }
}

fn validate_relative_path(raw: &str) -> Result<PathBuf, Failure> {
    let path = Path::new(raw);
    if raw.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Failure::new(
            "outside_project",
            "路径必须是当前项目内的相对路径。",
        ));
    }
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("drawio"))
    {
        return Err(Failure::new(
            "unsupported_format",
            "AI Draw.io 工具首期只支持 .drawio 文件。",
        ));
    }
    Ok(path.to_path_buf())
}

fn existing_path(root: &Path, relative: &Path) -> Result<PathBuf, Failure> {
    let path = files::project_path(root, &relative.to_string_lossy()).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            "file_not_found"
        } else if error.kind() == std::io::ErrorKind::PermissionDenied {
            "outside_project"
        } else {
            "invalid_path"
        };
        Failure::new(code, error.to_string())
    })?;
    if !path.is_file() {
        return Err(Failure::new(
            "file_not_found",
            "指定的 .drawio 文件不存在。",
        ));
    }
    Ok(path)
}

fn new_path(root: &Path, relative: &Path) -> Result<PathBuf, Failure> {
    let parent = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = files::project_path(root, &parent.to_string_lossy()).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "outside_project"
        } else {
            "directory_not_found"
        };
        Failure::new(code, error.to_string())
    })?;
    if !parent.is_dir() {
        return Err(Failure::new("directory_not_found", "目标目录不存在。"));
    }
    let name = relative
        .file_name()
        .ok_or_else(|| Failure::new("invalid_path", "路径必须包含文件名。"))?;
    Ok(parent.join(name))
}

fn validate_xml(xml: &str) -> Result<(), Failure> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| Failure::new("invalid_xml", error.to_string()))?;
    if document.root_element().tag_name().name() != "mxfile" {
        return Err(Failure::new(
            "invalid_xml",
            "Draw.io XML 的根元素必须是 mxfile。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Operation, Params, Request, execute};
    use std::fs;

    fn request(operation: Operation, path: &str, xml: Option<&str>) -> Request {
        Request {
            operation,
            params: Params {
                path: Some(path.into()),
                xml: xml.map(str::to_owned),
                operations: None,
            },
        }
    }

    #[test]
    fn create_read_and_update_keep_native_drawio_xml_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let original = "<mxfile><diagram id=\"one\"/></mxfile>";
        let replacement = "<mxfile><diagram id=\"two\"/></mxfile>";

        assert_eq!(
            execute(
                root.path(),
                request(Operation::Create, "diagram.drawio", Some(original))
            )
            .unwrap()["status"],
            "ok"
        );
        assert_eq!(
            execute(
                root.path(),
                request(Operation::Read, "diagram.drawio", None)
            )
            .unwrap()["xml"],
            original
        );
        assert_eq!(
            execute(
                root.path(),
                request(Operation::Update, "diagram.drawio", Some(replacement))
            )
            .unwrap()["status"],
            "ok"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("diagram.drawio")).unwrap(),
            replacement
        );
    }

    #[test]
    fn create_refuses_existing_files_and_invalid_xml() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("diagram.drawio"), "<mxfile/>").unwrap();

        assert_eq!(
            execute(
                root.path(),
                request(Operation::Create, "diagram.drawio", Some("<mxfile/>"))
            )
            .unwrap_err()
            .code,
            "file_exists"
        );
        assert_eq!(
            execute(
                root.path(),
                request(Operation::Create, "bad.drawio", Some("<mxfile>"))
            )
            .unwrap_err()
            .code,
            "invalid_xml"
        );
        assert!(!root.path().join("bad.drawio").exists());
    }

    #[test]
    fn update_rejects_paths_outside_the_project_and_non_drawio_files() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(
            execute(
                root.path(),
                request(Operation::Read, "../outside.drawio", None)
            )
            .unwrap_err()
            .code,
            "outside_project"
        );
        assert_eq!(
            execute(
                root.path(),
                request(Operation::Create, "diagram.drawio.svg", Some("<mxfile/>"))
            )
            .unwrap_err()
            .code,
            "unsupported_format"
        );
    }
}
