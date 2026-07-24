//! Prompt pack: a directory of zerostack prompt files evaluated as one unit.
//!
//! zerostack merges prompts from four layers into one map keyed by prompt
//! name (`src/context/prompts.rs`), the last of which is `./.zerostack/prompts`
//! under the working directory. It reads only the *top level* of such a
//! directory, only `*.md`, never recursively, and takes each file's stem as
//! the prompt name (`src/context/mod.rs`). A pack is therefore exactly the set
//! of files zerostack would read, and anything else in the directory is
//! rejected at load time rather than copied where nothing will look at it —
//! silently skipping it would reproduce the "the file is there but nothing
//! loads it" failure this whole capability exists to prevent.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// One prompt file in a pack.
#[derive(Debug)]
pub struct PackFile {
    /// The file as it must land in `.zerostack/prompts/`, e.g. `code.md`.
    pub file_name: String,
    /// The prompt name zerostack resolves it under: the stem, e.g. `code`.
    pub name: String,
    /// Contents, read once at load time — a pack is a handful of small files,
    /// and the fingerprint needs the bytes anyway.
    pub bytes: Vec<u8>,
}

/// A validated prompt pack. Construction is the validation: holding one means
/// the directory contained nothing but top-level `*.md`, and at least one.
#[derive(Debug)]
pub struct PromptPack {
    dir: PathBuf,
    /// Sorted by `file_name`, so every consumer sees one stable order.
    files: Vec<PackFile>,
}

impl PromptPack {
    /// Read and validate `dir` as a prompt pack. Every rejection names the
    /// offending entry: a pack is authored by hand, so the error has to say
    /// which file to move, not merely that something is wrong.
    pub fn load(dir: &Path) -> Result<PromptPack> {
        if !dir.exists() {
            bail!("prompt pack '{}': no such directory", dir.display());
        }
        if !dir.is_dir() {
            bail!(
                "prompt pack '{}': not a directory — --prompts takes a directory of \
                 top-level *.md prompt files",
                dir.display()
            );
        }

        let mut files: Vec<PackFile> = Vec::new();
        let mut rejected: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if path.is_dir() {
                rejected.push(format!("{file_name}/ (a subdirectory)"));
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                rejected.push(file_name);
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            files.push(PackFile {
                file_name,
                name,
                bytes: std::fs::read(&path)?,
            });
        }

        if !rejected.is_empty() {
            rejected.sort();
            bail!(
                "prompt pack '{}': zerostack reads only the top-level *.md files of a \
                 prompt directory, so these entries could never be loaded: {} — move \
                 them out of the pack",
                dir.display(),
                rejected.join(", ")
            );
        }
        if files.is_empty() {
            bail!(
                "prompt pack '{}': contains no *.md file, so it could not override any \
                 prompt",
                dir.display()
            );
        }

        files.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        Ok(PromptPack {
            dir: dir.to_path_buf(),
            files,
        })
    }

    /// The directory this pack was loaded from, as given on the command line.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The prompt names this pack provides (file stems), sorted.
    pub fn names(&self) -> Vec<&str> {
        self.files.iter().map(|f| f.name.as_str()).collect()
    }

    /// The pack's files, sorted by file name — what seeding copies.
    pub fn files(&self) -> &[PackFile] {
        &self.files
    }

    /// FNV-1a fingerprint over the pack's contents *and* its file names,
    /// folded as `name\0bytes\0` in file-name order — the same hashing
    /// approach `Scenario::content_hash` and `Report::judge_hash` already use,
    /// so the repo has one hash, not two.
    ///
    /// Sorting here, rather than trusting the order `load` happened to build,
    /// is what pins "filesystem enumeration order cannot change a pack's
    /// identity". Names count because renaming a prompt changes which built-in
    /// it overrides, which is a behavior change with no byte of content moved.
    /// The pack directory's own path does not count, so an unchanged pack that
    /// moved keeps its identity.
    pub fn fingerprint(&self) -> String {
        let mut ordered: Vec<&PackFile> = self.files.iter().collect();
        ordered.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        let mut buf: Vec<u8> = Vec::new();
        for f in ordered {
            buf.extend_from_slice(f.file_name.as_bytes());
            buf.push(0);
            buf.extend_from_slice(&f.bytes);
            buf.push(0);
        }
        crate::util::fnv1a_hex(&buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh empty directory named after the test, so parallel tests in
    /// this process never share one.
    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zseval-prompts-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn load_reads_top_level_md_files_sorted_by_name() {
        let dir = tmp("load-sorted");
        write(&dir, "review.md", "review body");
        write(&dir, "code.md", "code body");
        let pack = PromptPack::load(&dir).unwrap();
        assert_eq!(pack.names(), vec!["code", "review"]);
        assert_eq!(pack.files()[0].file_name, "code.md");
        assert_eq!(pack.files()[0].bytes, b"code body");
        assert_eq!(pack.files()[1].file_name, "review.md");
        assert_eq!(pack.dir(), dir.as_path());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rejects_a_missing_path() {
        let err = PromptPack::load(Path::new("/no/such/prompt-pack")).unwrap_err();
        assert!(
            format!("{err:#}").contains("/no/such/prompt-pack"),
            "{err:#}"
        );
    }

    #[test]
    fn load_rejects_a_path_that_is_not_a_directory() {
        let dir = tmp("not-a-dir");
        let file = dir.join("code.md");
        std::fs::write(&file, "body").unwrap();
        let err = PromptPack::load(&file).unwrap_err();
        assert!(format!("{err:#}").contains("not a directory"), "{err:#}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rejects_a_directory_with_no_md_file() {
        let dir = tmp("empty");
        let err = PromptPack::load(&dir).unwrap_err();
        assert!(format!("{err:#}").contains("no *.md file"), "{err:#}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rejects_a_subdirectory_naming_it() {
        let dir = tmp("subdir");
        write(&dir, "code.md", "code body");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        let err = PromptPack::load(&dir).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("nested"), "{msg}");
        assert!(msg.contains("top-level *.md"), "{msg}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rejects_a_non_markdown_file_naming_it() {
        let dir = tmp("non-md");
        write(&dir, "code.md", "code body");
        write(&dir, "notes.txt", "notes");
        let err = PromptPack::load(&dir).unwrap_err();
        assert!(format!("{err:#}").contains("notes.txt"), "{err:#}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fingerprint_ignores_the_pack_directory_path() {
        let a = tmp("hash-move-a");
        let b = tmp("hash-move-b");
        for d in [&a, &b] {
            write(d, "code.md", "code body");
            write(d, "review.md", "review body");
        }
        assert_eq!(
            PromptPack::load(&a).unwrap().fingerprint(),
            PromptPack::load(&b).unwrap().fingerprint()
        );
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn fingerprint_changes_when_a_prompt_is_renamed() {
        let a = tmp("hash-rename-a");
        let b = tmp("hash-rename-b");
        write(&a, "code.md", "identical bytes");
        write(&b, "mycode.md", "identical bytes");
        assert_ne!(
            PromptPack::load(&a).unwrap().fingerprint(),
            PromptPack::load(&b).unwrap().fingerprint()
        );
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn fingerprint_ignores_file_enumeration_order() {
        // Built by hand rather than through `load`, which sorts: the point is
        // that the hash itself does not depend on what order the filesystem
        // handed the entries back in.
        let file = |file_name: &str, body: &str| PackFile {
            file_name: file_name.to_string(),
            name: file_name.trim_end_matches(".md").to_string(),
            bytes: body.as_bytes().to_vec(),
        };
        let forward = PromptPack {
            dir: PathBuf::from("p"),
            files: vec![file("code.md", "code body"), file("review.md", "review")],
        };
        let reversed = PromptPack {
            dir: PathBuf::from("p"),
            files: vec![file("review.md", "review"), file("code.md", "code body")],
        };
        assert_eq!(forward.fingerprint(), reversed.fingerprint());
    }
}
