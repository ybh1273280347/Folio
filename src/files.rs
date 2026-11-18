use std::{
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::model::{DiskDocument, FileKind, ProjectFile};

const TEXT_EXTENSIONS: &[&str] = &[
    "tex", "ltx", "bib", "sty", "cls", "bst", "bbx", "cbx", "cfg", "def", "clo", "ist", "glsdefs",
    "txt", "md", "csv", "tsv", "dat", "tikz", "pgf", "asy", "json", "yaml", "yml", "toml", "eps",
    "svg",
];
const ASSET_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "pdf", "html", "htm"];
pub const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".pi",
    "node_modules",
    ".idea",
    ".vscode",
    ".folio",
    "build",
    "dist",
    "target",
    "runtime",
];

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn is_relevant(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ".latexmkrc")
        || extension_in(path, TEXT_EXTENSIONS)
        || extension_in(path, ASSET_EXTENSIONS)
}

fn extension_in(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            extensions.contains(&extension.as_str())
        })
}

pub(crate) fn is_text(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ".latexmkrc") || extension_in(path, TEXT_EXTENSIONS)
}

pub fn project_path(root: &Path, relative: &str) -> io::Result<PathBuf> {
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "无法访问项目文件夹之外的文件。",
        ));
    }
    let canonical_root = dunce::canonicalize(root)?;
    let target = dunce::canonicalize(canonical_root.join(relative_path))?;
    if !target.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "无法访问项目文件夹之外的文件。",
        ));
    }
    Ok(target)
}

pub fn validate_folder(folder: &Path) -> io::Result<PathBuf> {
    let resolved = dunce::canonicalize(folder)?;
    if !resolved.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请选择一个文件夹。",
        ));
    }
    Ok(resolved)
}

pub fn list_files(root: &Path) -> io::Result<Vec<ProjectFile>> {
    fn visit(root: &Path, current: &Path, files: &mut Vec<ProjectFile>) -> io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir()
                && !EXCLUDED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
            {
                visit(root, &entry.path(), files)?;
            } else if file_type.is_file() && is_relevant(&entry.path()) {
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .expect("walked from project root")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push(ProjectFile {
                    kind: if is_text(&entry.path()) {
                        FileKind::Text
                    } else {
                        FileKind::Asset
                    },
                    path,
                });
            }
        }
        Ok(())
    }

    let root = dunce::canonicalize(root)?;
    let mut files = Vec::new();
    visit(&root, &root, &mut files)?;
    files.sort_by(|left, right| {
        left.path
            .to_lowercase()
            .cmp(&right.path.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(files)
}

fn revision(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn read_document(root: &Path, relative: &str) -> io::Result<DiskDocument> {
    read_document_at(&project_path(root, relative)?)
}

/// Reads a document directly by its canonical path for files in the default space.
pub fn read_document_at(path: &Path) -> io::Result<DiskDocument> {
    let bytes = fs::read(path)?;
    Ok(DiskDocument {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        revision: revision(&bytes),
    })
}

pub fn save_document(
    root: &Path,
    relative: &str,
    text: &str,
    expected_revision: &str,
) -> io::Result<Result<DiskDocument, DiskDocument>> {
    save_document_at(&project_path(root, relative)?, text, expected_revision)
}

/// Saves a default-space document only when its on-disk revision still matches.
pub fn save_document_at(
    path: &Path,
    text: &str,
    expected_revision: &str,
) -> io::Result<Result<DiskDocument, DiskDocument>> {
    let current = read_document_at(path)?;
    if current.revision != expected_revision {
        return Ok(Err(current));
    }
    fs::write(path, text.as_bytes())?;
    Ok(Ok(DiskDocument {
        text: text.to_owned(),
        revision: revision(text.as_bytes()),
    }))
}

pub fn create_file(root: &Path, relative: &str) -> io::Result<()> {
    let path = Path::new(relative);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if path.is_absolute()
        || !is_relevant(path)
        || name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|char| "<>:\"|?*".contains(char))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请输入有效的项目文件名。若需新建文本文件，请使用受支持的扩展名。",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || Ok(dunce::canonicalize(root)?),
            |parent| project_path(root, &parent.to_string_lossy()),
        )?;
    File::options()
        .write(true)
        .create_new(true)
        .open(parent.join(name))?;
    Ok(())
}

pub fn create_markdown_in(root: &Path, folder: Option<&str>) -> io::Result<ProjectFile> {
    let prefix = folder
        .map(|folder| folder.trim().trim_matches(['/', '\\']))
        .filter(|folder| !folder.is_empty());
    let mut number = 1;
    loop {
        let name = if number == 1 {
            "未命名.md".to_owned()
        } else {
            format!("未命名{number}.md")
        };
        let relative = match prefix {
            Some(folder) => format!("{folder}/{name}"),
            None => name,
        };
        match create_file(root, &relative) {
            Ok(()) => {
                return Ok(ProjectFile {
                    path: relative,
                    kind: FileKind::Text,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            Err(error) => return Err(error),
        }
    }
}

pub fn rename_file(root: &Path, relative: &str, name: &str) -> io::Result<String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|char| char == '/' || char == '\\' || "<>:\"|?*".contains(char))
        || !is_relevant(Path::new(name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请输入有效的文件名和受支持的扩展名。",
        ));
    }
    let source = project_path(root, relative)?;
    let destination = source.with_file_name(name);
    fs::hard_link(&source, &destination)?;
    fs::remove_file(source)?;
    let parent = Path::new(relative)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    Ok(parent.map_or_else(
        || name.to_owned(),
        |parent| format!("{}/{}", parent.to_string_lossy().replace('\\', "/"), name),
    ))
}

pub fn list_folders(root: &Path) -> io::Result<Vec<String>> {
    fn visit(root: &Path, current: &Path, folders: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir()
                && !EXCLUDED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
            {
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .expect("walked from project root")
                    .to_string_lossy()
                    .replace('\\', "/");
                folders.push(path);
                visit(root, &entry.path(), folders)?;
            }
        }
        Ok(())
    }

    let root = dunce::canonicalize(root)?;
    let mut folders = Vec::new();
    visit(&root, &root, &mut folders)?;
    folders.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    Ok(folders)
}

pub fn create_folder(root: &Path, relative: &str) -> io::Result<()> {
    let relative = relative.trim().trim_matches(['/', '\\']);
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || relative.chars().any(|char| "<>:\"|?*".contains(char))
        || path.components().any(|part| {
            EXCLUDED_DIRECTORIES
                .iter()
                .any(|name| part.as_os_str() == *name)
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请输入有效的文件夹名称。",
        ));
    }
    let canonical_root = dunce::canonicalize(root)?;
    let target = canonical_root.join(path);
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "同名文件夹或文件已存在。",
        ));
    }
    fs::create_dir_all(target)
}

/// Move a whole directory within one canonical project root, without merging or replacing a target.
pub fn move_folder_into(root: &Path, relative: &str, folder: &str) -> io::Result<String> {
    let root = dunce::canonicalize(root)?;
    let source = project_path(&root, relative)?;
    let destination = project_path(&root, if folder.is_empty() { "." } else { folder })?;
    if source == root || !source.is_dir() || !destination.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "请选择项目内的文件夹。"));
    }
    if destination.starts_with(&source) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "不能将文件夹移入自身或子文件夹。"));
    }
    // Both existing paths were resolved inside root; appending one filename keeps the target inside it too.
    let target = destination.join(source.file_name().expect("non-root folder has a name"));
    if source == target {
        return Ok(relative.to_owned());
    }
    if target.try_exists()? {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "目标已存在同名文件或文件夹，未移动。"));
    }
    fs::rename(&source, &target)?;
    Ok(target.strip_prefix(&root).expect("validated project destination").to_string_lossy().replace('\\', "/"))
}

pub fn rename_folder(root: &Path, relative: &str, name: &str) -> io::Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|char| char == '/' || char == '\\' || "<>:\"|?*".contains(char))
        || EXCLUDED_DIRECTORIES.contains(&name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请输入有效的文件夹名称。",
        ));
    }
    let source = project_path(root, relative)?;
    if !source.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请选择项目内的文件夹。",
        ));
    }
    let current_name = source
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or_default();
    let parent = Path::new(relative)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let renamed = parent.map_or_else(
        || name.to_owned(),
        |parent| format!("{}/{}", parent.to_string_lossy().replace('\\', "/"), name),
    );
    if current_name == name {
        return Ok(renamed);
    }
    let destination = source.with_file_name(name);
    if !current_name.eq_ignore_ascii_case(name) && destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "同名文件夹或文件已存在。",
        ));
    }
    fs::rename(&source, &destination)?;
    Ok(renamed)
}

#[cfg(target_os = "windows")]
fn files_app_executable() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let is_files_default = [
        r"HKCU\Software\Classes\Folder\shell\open\command",
        r"HKCU\Software\Classes\Directory\shell\open\command",
    ]
    .into_iter()
    .any(|key| {
        Command::new("reg")
            .args(["query", key, "/ve"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
                stdout.contains("files.app.launcher.exe")
                    || stdout.contains("files-stable")
                    || stdout.contains("files-preview")
                    || stdout.contains("files-dev")
            })
    });
    if !is_files_default {
        return None;
    }

    let local_app_data = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let windows_apps = local_app_data.join("Microsoft").join("WindowsApps");

    if let Ok(branch) = fs::read_to_string(local_app_data.join("Files").join("Branch.txt")) {
        let alias = branch.trim();
        if !alias.is_empty() {
            let candidate = windows_apps.join(format!("{alias}.exe"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    for alias in ["files-stable.exe", "files-preview.exe", "files-dev.exe"] {
        let candidate = windows_apps.join(alias);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn reveal_path_windows(path: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    if path.as_os_str().is_empty() {
        return;
    }
    let target = path.to_path_buf();
    std::thread::spawn(move || {
        let normalized = dunce::canonicalize(&target)
            .unwrap_or_else(|_| PathBuf::from(target.to_string_lossy().replace('/', "\\")));
        let has_parent = normalized.parent().is_some();

        if let Some(files_exe) = files_app_executable() {
            let mut command = Command::new(&files_exe);
            if has_parent {
                command.arg("-select").arg(&normalized);
            } else {
                command.arg(&normalized);
            }
            if command.spawn().is_ok() {
                return;
            }
        }

        let mut explorer = Command::new("explorer.exe");
        if has_parent {
            let escaped = normalized.to_string_lossy().replace('"', "");
            explorer.raw_arg(format!("/select,\"{escaped}\""));
        } else {
            explorer.arg(&normalized);
        }
        let _ = explorer.spawn();
    });
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn quick_notes_number_without_overwriting_existing_content() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("未命名.md"), "existing").unwrap();
        fs::write(temp.path().join("未命名3.md"), "third").unwrap();
        assert_eq!(create_markdown_in(temp.path(), None).unwrap().path, "未命名2.md");
        assert_eq!(create_markdown_in(temp.path(), None).unwrap().path, "未命名4.md");
        assert_eq!(fs::read_to_string(temp.path().join("未命名.md")).unwrap(), "existing");
    }

    #[test]
    fn renamed_markdown_keeps_body_and_accepts_unsaved_draft() {
        let temp = tempdir().unwrap();
        let note = create_markdown_in(temp.path(), None).unwrap();
        fs::write(temp.path().join(&note.path), "# Independent heading\n\nbody").unwrap();
        let disk = read_document(temp.path(), &note.path).unwrap();
        let renamed = rename_file(temp.path(), &note.path, "读书笔记.md").unwrap();
        assert_eq!(read_document(temp.path(), &renamed).unwrap().text, disk.text);
        save_document(temp.path(), &renamed, "unsaved draft", &disk.revision).unwrap().unwrap();
        assert_eq!(fs::read_to_string(temp.path().join(&renamed)).unwrap(), "unsaved draft");
        assert!(!temp.path().join(&note.path).exists());
        fs::write(temp.path().join("occupied.md"), "keep").unwrap();
        assert!(rename_file(temp.path(), &renamed, "occupied.md").is_err());
        assert_eq!(fs::read_to_string(temp.path().join("occupied.md")).unwrap(), "keep");
        assert!(temp.path().join(renamed).exists());
    }

    #[test]
    fn listing_filters_build_directories_and_sorts_nested_supported_files() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("chapters")).unwrap();
        fs::create_dir_all(temp.path().join("build")).unwrap();
        fs::write(temp.path().join("chapters/intro.tex"), "source").unwrap();
        fs::write(temp.path().join("image.png"), "asset").unwrap();
        fs::write(temp.path().join("build/ignored.tex"), "generated").unwrap();
        fs::write(temp.path().join("notes.exe"), "unsupported").unwrap();

        let files = list_files(temp.path()).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| (file.path.as_str(), file.kind))
                .collect::<Vec<_>>(),
            [
                ("chapters/intro.tex", FileKind::Text),
                ("image.png", FileKind::Asset),
            ]
        );
    }

    #[test]
    fn save_compares_the_current_disk_revision_before_writing() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("main.tex"), "first").unwrap();
        let original = read_document(temp.path(), "main.tex").unwrap();
        fs::write(temp.path().join("main.tex"), "external").unwrap();

        let conflict = save_document(temp.path(), "main.tex", "draft", &original.revision)
            .unwrap()
            .unwrap_err();
        assert_eq!(conflict.text, "external");
        assert_eq!(
            fs::read_to_string(temp.path().join("main.tex")).unwrap(),
            "external"
        );
        assert_eq!(
            save_document(temp.path(), "main.tex", "draft", &conflict.revision)
                .unwrap()
                .unwrap()
                .text,
            "draft"
        );
    }

    #[test]
    fn paths_cannot_escape_the_selected_project() {
        let temp = tempdir().unwrap();
        let outside = temp.path().join("outside.tex");
        fs::write(&outside, "secret").unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        assert_eq!(
            project_path(&project, "../outside.tex").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(project_path(&project, "outside.tex").is_err());
    }

    #[test]
    fn folder_rename_preserves_contents_and_rejects_invalid_names() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("parent/old_dir")).unwrap();
        fs::create_dir_all(temp.path().join("parent/occupied")).unwrap();
        fs::write(temp.path().join("parent/old_dir/note.md"), "hello").unwrap();

        let renamed = rename_folder(temp.path(), "parent/old_dir", "new_dir").unwrap();
        assert_eq!(renamed, "parent/new_dir");
        assert!(!temp.path().join("parent/old_dir").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("parent/new_dir/note.md")).unwrap(),
            "hello"
        );
        assert!(rename_folder(temp.path(), &renamed, "occupied").is_err());
        assert!(rename_folder(temp.path(), &renamed, "bad/name").is_err());
        assert!(rename_folder(temp.path(), &renamed, ".git").is_err());
    }

    #[test]
    fn percent_decode_decodes_utf8_and_preserves_plus() {
        assert_eq!(percent_decode("hello%20world+1.md"), "hello world+1.md");
        assert_eq!(percent_decode("%E7%AC%94%E8%AE%B0.md"), "笔记.md");
        assert_eq!(percent_decode("bad%ZZ%2"), "bad%ZZ%2");
    }
}
