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
const ASSET_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "pdf"];
pub const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    "node_modules",
    ".idea",
    ".vscode",
    ".folio",
    "build",
    "dist",
    "target",
    "runtime",
];

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

fn is_text(path: &Path) -> bool {
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
    let bytes = fs::read(project_path(root, relative)?)?;
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
    let current = read_document(root, relative)?;
    if current.revision != expected_revision {
        return Ok(Err(current));
    }
    fs::write(project_path(root, relative)?, text.as_bytes())?;
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

pub fn delete_file(root: &Path, relative: &str) -> io::Result<()> {
    fs::remove_file(project_path(root, relative)?)
}

pub fn delete_folder_files(root: &Path, relative: &str) -> io::Result<()> {
    let folder = project_path(root, relative)?;
    if !folder.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请选择项目内的文件夹。",
        ));
    }
    for file in list_files(&folder)? {
        fs::remove_file(project_path(&folder, &file.path)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::*;

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
    fn folder_delete_removes_only_supported_files() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested/source.md"), "text").unwrap();
        fs::write(temp.path().join("nested/keep.bin"), "keep").unwrap();

        delete_folder_files(temp.path(), "nested").unwrap();
        assert!(!Path::new(&temp.path().join("nested/source.md")).exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("nested/keep.bin")).unwrap(),
            "keep"
        );
    }
}
