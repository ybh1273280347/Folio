//! Native Draw.io file operations and their tool request format.

use crate::files;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn result(self) -> Value {
        json!({ "status": "error", "error": { "code": self.code, "message": self.message } })
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Read,
    Create,
    Update,
    GetSelection,
    EditCells,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Params {
    path: Option<String>,
    xml: Option<String>,
    operations: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub operation: Operation,
    pub params: Params,
}

pub(crate) fn execute(root: &Path, request: Request) -> Result<Value, Failure> {
    let path = request.params.path.as_deref()
        .ok_or_else(|| Failure::new("invalid_request", "文件操作必须提供 path。"))?;
    let relative = validate_relative_path(path)?;
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
