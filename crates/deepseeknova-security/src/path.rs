use deepseeknova_core::DeepseeknovaError;
use std::path::{Path, PathBuf};

/// Normalize path components by resolving `.` and `..` without hitting the disk.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            _ => {
                normalized.push(component);
            }
        }
    }
    normalized
}

/// Resolve and validate a user-supplied path against a root workspace directory,
/// preventing path traversal and symlink escape attacks.
pub fn secure_resolve(root: &Path, input: &Path) -> Result<PathBuf, DeepseeknovaError> {
    // Resolve absolute path or join with root if relative
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };

    // 1. Raw memory normalization check (stops basic .. traversal)
    let normalized = normalize_path(&joined);

    if !normalized.starts_with(root) {
        tracing::warn!(
            security_event = "path_escape_attempt",
            requested = ?input.display(),
            resolved = ?normalized.display(),
            workspace = ?root.display(),
            reason = "escapes workspace root via raw memory check"
        );
        return Err(DeepseeknovaError::permission(format!(
            "path escapes workspace root (normalization check): {:?}",
            input
        )));
    }

    // 2. Canonicalize the closest existing ancestor (or symlink) to prevent symlink escape
    let mut current = normalized.as_path();
    while !std::fs::symlink_metadata(current).is_ok() {
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }

    if std::fs::symlink_metadata(current).is_ok() {
        let canonical = match std::fs::canonicalize(current) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    security_event = "path_validation_failed",
                    requested = ?input.display(),
                    error = %e,
                    reason = "canonicalize failed"
                );
                return Err(DeepseeknovaError::Io(e));
            }
        };
        // On Windows, canonicalize returns a UNC path (\\?\C:\...).
        // Normalize root the same way for a fair comparison.
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !canonical.starts_with(&canonical_root) {
            tracing::warn!(
                security_event = "path_escape_attempt",
                requested = ?input.display(),
                resolved = ?canonical.display(),
                workspace = ?canonical_root.display(),
                reason = "escapes workspace root via symlink"
            );
            return Err(DeepseeknovaError::permission(format!(
                "path escapes workspace root via symlink: {:?}",
                input
            )));
        }
    }

    Ok(normalized)
}

/// 检测「工作区根目录疑似错误」：当前工作目录是否是 `target/` 之类的构建
/// 产物子目录（如 `target/release`），这类目录里运行工具会因 `path escapes
/// workspace root` 大量失败，agent 被迫回退 `bash sed/cat` 并烧干 step 配额。
///
/// 返回 `Some(hint)` 表示疑似误设（附提示文案），`None` 表示正常。纯启发式，
/// 不阻断（`target` 作为合法 workspace 的场景少，但存在——如独立前端目标）。
pub fn detect_suspicious_workspace_root(cwd: &Path) -> Option<String> {
    let file_name = cwd.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    // `release`/`debug`/`bench`/`deps` 等 cargo 产物目录通常嵌套在 target 下。
    let looks_like_build_out = matches!(
        file_name,
        "release" | "debug" | "bench" | "deps" | "build" | "incremental"
    );
    if !looks_like_build_out {
        return None;
    }
    // 只有父目录名也为 `target`（或其下级）时才判定为「从构建产物目录启动」。
    let parent_is_target = cwd
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .is_some_and(|p| p == "target");
    if !parent_is_target {
        return None;
    }
    Some(format!(
        "当前工作目录是构建产物目录 `{}`，可能导致文件工具报 `path escapes workspace root` \
         并回退到低效的 bash 命令。请从项目根目录启动。",
        cwd.display()
    ))
}

/// Basic path sanitization: reject obviously malicious paths
/// and ensure the path stays within the workspace root.
pub fn sanitize_path(workspace: &Path, raw: &str) -> Result<PathBuf, DeepseeknovaError> {
    if raw.is_empty() {
        return Err(DeepseeknovaError::permission("empty path"));
    }
    if raw.contains('\0') {
        return Err(DeepseeknovaError::permission("path contains null byte"));
    }
    secure_resolve(workspace, Path::new(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── detect_suspicious_workspace_root ──────────────────────────

    #[test]
    fn detects_build_output_directory() {
        assert!(
            detect_suspicious_workspace_root(Path::new("/proj/target/release")).is_some(),
            "target/release 应被判为疑似误设工作区"
        );
        assert!(
            detect_suspicious_workspace_root(Path::new("/proj/target/debug")).is_some(),
            "target/debug 应被判为疑似误设工作区"
        );
    }

    #[test]
    fn ignores_normal_workspace_roots() {
        assert!(detect_suspicious_workspace_root(Path::new("/proj")).is_none());
        assert!(detect_suspicious_workspace_root(Path::new("/proj/src")).is_none());
        // release 目录名但父级不是 target → 不误判。
        assert!(detect_suspicious_workspace_root(Path::new("/proj/release")).is_none());
    }

    // ── normalize_path ───────────────────────────────────────────

    #[test]
    fn test_normalize_path_noop() {
        let p = normalize_path(Path::new("/a/b/c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn test_normalize_path_removes_dot() {
        let p = normalize_path(Path::new("/a/./b"));
        assert_eq!(p, PathBuf::from("/a/b"));
    }

    #[test]
    fn test_normalize_path_resolves_dotdot() {
        let p = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn test_normalize_path_escape_attempt() {
        let p = normalize_path(Path::new("/a/b/../../etc/passwd"));
        assert_eq!(p, PathBuf::from("/etc/passwd"));
    }

    // ── secure_resolve ───────────────────────────────────────────

    #[test]
    fn test_secure_resolve_normal_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let res = secure_resolve(&root, Path::new("src/main.rs")).unwrap();
        assert_eq!(res, root.join("src/main.rs"));
    }

    #[test]
    fn test_secure_resolve_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let res = secure_resolve(&root, Path::new("../../etc/passwd"));
        assert!(res.is_err(), "should block traversal escape");
    }

    #[test]
    fn test_secure_resolve_rejects_absolute_path_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let res = secure_resolve(&root, Path::new("/etc"));
        assert!(res.is_err(), "should block absolute path escape");
    }

    #[test]
    fn test_secure_resolve_allows_deep_nested_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        // Create deep nested dir
        let deep = root.join("a/b/c/d");
        std::fs::create_dir_all(&deep).unwrap();
        let res = secure_resolve(&root, Path::new("a/b/c/d/file.txt")).unwrap();
        assert_eq!(res, deep.join("file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn test_secure_resolve_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();

        let outside_tmp = tempfile::tempdir().unwrap();
        let outside_path = std::fs::canonicalize(outside_tmp.path()).unwrap();
        let link_path = root.join("bad_symlink");

        if symlink(&outside_path, &link_path).is_ok() {
            let res = secure_resolve(&root, Path::new("bad_symlink/some_file"));
            assert!(res.is_err(), "should block symlink escape: {:?}", res);
        }
    }

    // ── sanitize_path ────────────────────────────────────────────

    #[test]
    fn test_sanitize_path_rejects_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let res = sanitize_path(tmp.path(), "");
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("empty path"));
    }

    #[test]
    fn test_sanitize_path_rejects_null_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let res = sanitize_path(tmp.path(), "safe\0.txt");
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("null byte"));
    }

    #[test]
    fn test_sanitize_path_accepts_valid_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let res = sanitize_path(&root, "valid/file.txt").unwrap();
        assert_eq!(res, root.join("valid/file.txt"));
    }
}
