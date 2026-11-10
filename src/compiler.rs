use std::{
    fs,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    files,
    model::{BuildStatus, Project},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct BuildResult {
    pub status: BuildStatus,
    pub elapsed: Duration,
    pub log: String,
    pub preview: Option<PathBuf>,
}

pub fn compile_tex(
    project: &Project,
    build_root: &Path,
    cancelled: Arc<AtomicBool>,
    on_log: impl FnMut(&str),
) -> io::Result<BuildResult> {
    if project.main_file.is_empty()
        || Path::new(&project.main_file)
            .extension()
            .is_none_or(|extension| extension.to_string_lossy().to_lowercase() != "tex")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "主文件必须是 .tex 文件。",
        ));
    }
    let source = files::project_path(Path::new(&project.folder), &project.main_file)?;
    run_latexmk(
        project,
        &source,
        &source.with_extension("pdf"),
        build_root,
        cancelled,
        on_log,
    )
}

fn run_latexmk(
    project: &Project,
    source: &Path,
    destination: &Path,
    build_root: &Path,
    cancelled: Arc<AtomicBool>,
    mut on_log: impl FnMut(&str),
) -> io::Result<BuildResult> {
    let output = build_root.join(&project.id).join("work");
    fs::create_dir_all(&output)?;
    let pdf_name = source
        .file_stem()
        .expect("TeX source has a file stem")
        .to_string_lossy()
        .to_string();
    let pdf = output.join(format!("{pdf_name}.pdf"));
    // latexmk owns output freshness; deleting its PDF defeats incremental builds.
    let started = Instant::now();
    if cancelled.load(Ordering::Relaxed) {
        return Ok(BuildResult {
            status: BuildStatus::Cancelled,
            elapsed: Duration::ZERO,
            log: String::new(),
            preview: None,
        });
    }

    let mut command = Command::new("latexmk");
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW, also for latexmk's child console.
    let mut child = command
        .args([
            project.engine.latexmk_flag(),
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-file-line-error",
            "-no-shell-escape",
        ])
        .arg(format!("-outdir={}", output.display()))
        .arg(source)
        .current_dir(&project.folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let (sender, receiver) = mpsc::channel();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stdout_sender = sender.clone();
    let stdout_reader = thread::spawn(move || forward_output(stdout, stdout_sender));
    let stderr_reader = thread::spawn(move || forward_output(stderr, sender));
    let mut log = String::new();
    let exit = loop {
        for line in receiver.try_iter() {
            log.push_str(&line);
            on_log(&line);
        }
        if cancelled.load(Ordering::Relaxed) {
            terminate_process_tree(&mut child, pid);
            break None;
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        thread::sleep(Duration::from_millis(40));
    };
    stdout_reader
        .join()
        .expect("stdout reader thread panicked")?;
    stderr_reader
        .join()
        .expect("stderr reader thread panicked")?;
    for line in receiver.try_iter() {
        log.push_str(&line);
        on_log(&line);
    }
    if exit.is_none() {
        return Ok(BuildResult {
            status: BuildStatus::Cancelled,
            elapsed: started.elapsed(),
            log,
            preview: None,
        });
    }
    if !exit.expect("checked above").success() {
        return Ok(BuildResult {
            status: BuildStatus::Failed,
            elapsed: started.elapsed(),
            log,
            preview: None,
        });
    }

    let preview = build_root.join(&project.id).join("preview.pdf");
    fs::copy(&pdf, &preview)?;
    fs::copy(&pdf, destination)?;
    Ok(BuildResult {
        status: BuildStatus::Success,
        elapsed: started.elapsed(),
        log,
        preview: Some(preview),
    })
}

fn forward_output(mut output: impl Read, sender: mpsc::Sender<String>) -> io::Result<()> {
    let mut reader = BufReader::new(&mut output);
    let mut bytes = Vec::new();
    while reader.read_until(b'\n', &mut bytes)? > 0 {
        let line = String::from_utf8_lossy(&bytes).into_owned();
        if sender.send(line).is_err() {
            return Ok(());
        }
        bytes.clear();
    }
    Ok(())
}

fn terminate_process_tree(child: &mut Child, pid: u32) {
    if cfg!(target_os = "windows") {
        let mut command = Command::new("taskkill");
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let _ = command
            .args(["/pid", &pid.to_string(), "/T", "/F"])
            .status();
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This acceptance check runs real TeX engines; keep it opt-in so normal
    /// file-operation checks do not repeatedly rebuild documents.
    #[test]
    #[ignore = "requires installed TeX Live"]
    fn real_tex_and_markdown_publish_next_to_their_sources() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("project with spaces");
        fs::create_dir_all(folder.join("nested")).unwrap();
        fs::write(
            folder.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}Folio native compilation.\\end{document}",
        )
        .unwrap();
        fs::write(
            folder.join("nested/notes.md"),
            "# Folio\n\n中文导出与 English content.\n",
        )
        .unwrap();
        let mut project = Project {
            id: "smoke".into(),
            name: "smoke".into(),
            folder: folder.to_string_lossy().into_owned(),
            main_file: "main.tex".into(),
            engine: Engine::Xelatex,
        };
        let build = temp.path().join("builds");
        for engine in [Engine::Xelatex, Engine::Pdflatex, Engine::Lualatex] {
            project.engine = engine;
            let result =
                compile_tex(&project, &build, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
            assert_eq!(
                result.status,
                BuildStatus::Success,
                "{engine:?}: {}",
                result.log
            );
            assert!(
                fs::read(folder.join("main.pdf"))
                    .unwrap()
                    .starts_with(b"%PDF-")
            );
        }
        let result = compile_markdown(
            &project,
            "nested/notes.md",
            &build,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.status, BuildStatus::Success, "{}", result.log);
        assert!(
            fs::read(folder.join("nested/notes.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let published = fs::read(result.preview.unwrap()).unwrap();
        fs::write(
            folder.join("main.tex"),
            "\\documentclass{article}\\begin{document}\\undefinedfolio\\end{document}",
        )
        .unwrap();
        let failed =
            compile_tex(&project, &build, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(failed.status, BuildStatus::Failed);
        assert_eq!(
            fs::read(build.join("smoke/preview.pdf")).unwrap(),
            published
        );
    }
}
