//! # tremolite-packaging
//!
//! 读取、验证、安装 `.amod` 模块包。
//!
//! 遵循 Artic Protocol Section 10 — Module Package Format 规范。
//!
//! ## 快速使用
//!
//! ```rust,no_run
//! use tremolite_packaging::{PackageReader, ModuleInstaller};
//!
//! // 读取 .amod 包
//! let pkg = PackageReader::from_file("emotion-detect-v1.0.0.amod")?;
//! println!("Module: {} v{}", pkg.manifest().module.name, pkg.manifest().module.version);
//!
//! // 安装到引擎模块目录
//! let installer = ModuleInstaller::new("/home/user/.artic/modules");
//! installer.install(&pkg)?;
//! # Ok::<_, anyhow::Error>(())
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ─── 错误类型 ────────────────────────────────────────

/// 包操作错误
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Archive error: {0}")]
    Archive(String),

    #[error("Manifest error: {0}")]
    Manifest(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Installation error: {0}")]
    Install(String),
}

// ─── 清单类型（对应 manifest.toml） ───────────────────

/// 完整清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub module: ManifestModule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<ManifestCompatibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<ManifestInstall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<ManifestSignature>,
}

/// 模块元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestModule {
    pub id: String,
    pub name: String,
    pub version: String,
    pub language: String,
    pub entry: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<ManifestAuthor>,
    #[serde(default)]
    pub declare: ManifestDeclare,
}

/// 作者信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAuthor {
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub url: String,
}

/// 声明（对应 Artic Protocol Declaration）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestDeclare {
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub handlers: Vec<String>,
}

/// 兼容性声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCompatibility {
    #[serde(default = "default_protocol")]
    pub min_protocol_version: String,
    #[serde(default)]
    pub engines: Vec<String>,
}

fn default_protocol() -> String { "draft-01".into() }

/// 安装配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestInstall {
    #[serde(default)]
    pub entry_args: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_install: Option<String>,
    /// 包内相对路径，模块安装目录
    #[serde(default = "default_install_dir")]
    pub dir: String,
}

fn default_install_dir() -> String { "module".into() }

/// 签名
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSignature {
    pub algorithm: String,
    pub value: String,
}

// ─── 已解析的包 ──────────────────────────────────────

/// 一个已解析的 `.amod` 包
pub struct ModulePackage {
    manifest: Manifest,
    /// 解压到的临时目录
    extract_dir: tempfile::TempDir,
    /// 源文件路径（如果是从文件读取的）
    source_path: Option<PathBuf>,
}

impl ModulePackage {
    /// 包清单引用
    pub fn manifest(&self) -> &Manifest { &self.manifest }
    /// 包清单可变引用
    pub fn manifest_mut(&mut self) -> &mut Manifest { &mut self.manifest }
    /// 解压目录（包内文件的根目录）
    pub fn extract_dir(&self) -> &Path { self.extract_dir.path() }
    /// 模块入口文件的绝对路径
    pub fn entry_path(&self) -> PathBuf {
        self.extract_dir.path().join(&self.manifest.module.entry)
    }
    /// 模块安装目录名（基于模块 ID）
    pub fn install_dir_name(&self) -> String {
        format!("{}-{}", self.manifest.module.id.replace('.', "-"), self.manifest.module.version)
    }
}

// ─── 包读取器 ────────────────────────────────────────

/// 从文件或字节流读取 `.amod` 包
pub struct PackageReader;

impl PackageReader {
    /// 从文件路径读取 `.amod` 包
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<ModulePackage, PackageError> {
        let path = path.as_ref();
        let data = fs::read(path)
            .map_err(|e| PackageError::Io(e))?;
        let mut pkg = Self::from_bytes(&data)?;
        pkg.source_path = Some(path.to_path_buf());
        Ok(pkg)
    }

    /// 从字节流读取 `.amod` 包
    pub fn from_bytes(data: &[u8]) -> Result<ModulePackage, PackageError> {
        let extract_dir = tempfile::tempdir()
            .map_err(|e| PackageError::Io(e))?;

        // 解压 tar.gz
        let decoder = flate2::read::GzDecoder::new(data);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(extract_dir.path())
            .map_err(|e| PackageError::Archive(format!("Failed to extract archive: {e}")))?;

        // 读取 manifest.toml
        let manifest_path = extract_dir.path().join("manifest.toml");
        if !manifest_path.exists() {
            return Err(PackageError::Manifest("manifest.toml not found in package".into()));
        }

        let manifest_content = fs::read_to_string(&manifest_path)
            .map_err(|e| PackageError::Io(e))?;
        let manifest: Manifest = toml::from_str(&manifest_content)
            .map_err(|e| PackageError::Manifest(format!("Invalid manifest.toml: {e}")))?;

        // 基本校验
        Self::validate(&manifest)?;

        Ok(ModulePackage { manifest, extract_dir, source_path: None })
    }

    /// 校验清单必需字段
    fn validate(manifest: &Manifest) -> Result<(), PackageError> {
        let m = &manifest.module;
        if m.id.is_empty() {
            return Err(PackageError::Validation("module.id is required".into()));
        }
        if m.name.is_empty() {
            return Err(PackageError::Validation("module.name is required".into()));
        }
        if m.version.is_empty() {
            return Err(PackageError::Validation("module.version is required".into()));
        }
        if m.entry.is_empty() {
            return Err(PackageError::Validation("module.entry is required".into()));
        }
        if m.declare.provides.is_empty() {
            return Err(PackageError::Validation("module.declare.provides must declare at least one service".into()));
        }
        Ok(())
    }
}

// ─── 安装器 ──────────────────────────────────────────

/// 模块安装器 —— 将 `.amod` 包安装到引擎模块目录
pub struct ModuleInstaller {
    /// 引擎的模块存放根目录（如 `~/.artic/modules/`）
    modules_dir: PathBuf,
}

impl ModuleInstaller {
    /// 创建安装器，指定引擎模块存放目录
    pub fn new<P: AsRef<Path>>(modules_dir: P) -> Self {
        Self { modules_dir: modules_dir.as_ref().to_path_buf() }
    }

    /// 安装包到引擎模块目录
    ///
    /// 返回安装后的模块目录路径。
    pub fn install(&self, pkg: &ModulePackage) -> Result<PathBuf, PackageError> {
        let dir_name = pkg.install_dir_name();
        let target_dir = self.modules_dir.join(&dir_name);

        // 如果已存在，先移除旧版本
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir)
                .map_err(|e| PackageError::Install(format!("Failed to remove existing module: {e}")))?;
        }

        // 创建目标目录的父目录
        fs::create_dir_all(&target_dir)
            .map_err(|e| PackageError::Install(format!("Failed to create module dir: {e}")))?;

        // 复制解压的文件到目标目录
        copy_dir(pkg.extract_dir(), &target_dir)?;

        // 执行 post_install 脚本（如果有）
        if let Some(ref post_script) = pkg.manifest().install.as_ref()
            .and_then(|i| i.post_install.as_ref())
        {
            let script_path = target_dir.join(post_script);
            if script_path.exists() {
                let status = std::process::Command::new("bash")
                    .arg(&script_path)
                    .current_dir(&target_dir)
                    .status()
                    .map_err(|e| PackageError::Install(format!("post_install script failed: {e}")))?;
                if !status.success() {
                    return Err(PackageError::Install("post_install script exited with non-zero status".into()));
                }
            }
        }

        tracing::info!("packaging: installed module '{}' v{} to {:?}",
            pkg.manifest().module.name, pkg.manifest().module.version, target_dir);

        Ok(target_dir)
    }

    /// 卸载已安装的模块
    pub fn uninstall(&self, module_id: &str) -> Result<(), PackageError> {
        // 扫描模块目录查找匹配的模块
        if !self.modules_dir.exists() {
            return Err(PackageError::Install(format!("Module '{}' not installed", module_id)));
        }

        let norm_id = module_id.replace('.', "-");
        let mut found = false;

        for entry in fs::read_dir(&self.modules_dir)
            .map_err(|e| PackageError::Io(e))? {
            let entry = entry.map_err(|e| PackageError::Io(e))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(&norm_id) {
                fs::remove_dir_all(entry.path())
                    .map_err(|e| PackageError::Install(format!("Failed to remove: {e}")))?;
                tracing::info!("packaging: uninstalled module '{}'", module_id);
                found = true;
            }
        }

        if !found {
            return Err(PackageError::Install(format!("Module '{}' not installed", module_id)));
        }
        Ok(())
    }

    /// 列出已安装的模块
    pub fn list_installed(&self) -> Result<Vec<InstalledModule>, PackageError> {
        let mut modules = Vec::new();

        if !self.modules_dir.exists() {
            return Ok(modules);
        }

        for entry in fs::read_dir(&self.modules_dir)
            .map_err(|e| PackageError::Io(e))? {
            let entry = entry.map_err(|e| PackageError::Io(e))?;
            let path = entry.path();
            if !path.is_dir() { continue; }

            // 读取该模块目录下的 manifest
            let manifest_path = path.join("manifest.toml");
            if manifest_path.exists() {
                if let Ok(content) = fs::read_to_string(&manifest_path) {
                    if let Ok(manifest) = toml::from_str::<Manifest>(&content) {
                        modules.push(InstalledModule {
                            id: manifest.module.id.clone(),
                            name: manifest.module.name.clone(),
                            version: manifest.module.version.clone(),
                            language: manifest.module.language.clone(),
                            entry: manifest.module.entry.clone(),
                            description: manifest.module.description.clone(),
                            provides: manifest.module.declare.provides.clone(),
                            path: path.clone(),
                            manifest: Some(manifest),
                        });
                        continue;
                    }
                }
            }
        }

        modules.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(modules)
    }

    /// 获取某模块的启动命令和参数
    pub fn launch_command(&self, module_id: &str) -> Result<(String, Vec<String>), PackageError> {
        let modules = self.list_installed()?;
        let installed = modules.into_iter()
            .find(|m| m.id == module_id)
            .ok_or_else(|| PackageError::Install(format!("Module '{}' not installed", module_id)))?;

        let entry_path = installed.path.join(&installed.entry);

        if !entry_path.exists() {
            return Err(PackageError::Install(format!(
                "Entry point not found: {}", entry_path.display())));
        }

        // 根据语言选择启动方式
        let (cmd, mut args) = match installed.language.to_lowercase().as_str() {
            "python" | "py" => {
                ("python3".into(), vec![entry_path.to_string_lossy().to_string()])
            }
            "node" | "javascript" | "js" | "typescript" | "ts" => {
                ("node".into(), vec![entry_path.to_string_lossy().to_string()])
            }
            "rust" | "rs" => {
                // Rust 模块需要先编译或用预编译二进制
                let binary = installed.path.join("module/main");
                if binary.exists() {
                    (binary.to_string_lossy().to_string(), vec![])
                } else {
                    return Err(PackageError::Install(
                        "Rust modules require a pre-built binary at module/main".into()));
                }
            }
            _ => {
                // 默认：尝试直接执行入口文件（如果可执行）
                (entry_path.to_string_lossy().to_string(), vec![])
            }
        };

        // 附加 entry_args
        if let Some(ref install_config) = installed.manifest.and_then(|m| m.install) {
            if !install_config.entry_args.is_empty() {
                for arg in install_config.entry_args.split_whitespace() {
                    args.push(arg.to_string());
                }
            }
        }

        Ok((cmd, args))
    }
}

/// 已安装模块的信息
#[derive(Debug, Clone)]
pub struct InstalledModule {
    pub id: String,
    pub name: String,
    pub version: String,
    pub language: String,
    pub entry: String,
    pub description: String,
    pub provides: Vec<String>,
    pub path: PathBuf,
    manifest: Option<Manifest>,
}

impl InstalledModule {
    fn from_path(path: &Path) -> Option<Self> {
        let manifest_path = path.join("manifest.toml");
        let content = fs::read_to_string(&manifest_path).ok()?;
        let manifest: Manifest = toml::from_str(&content).ok()?;
        Some(Self {
            id: manifest.module.id.clone(),
            name: manifest.module.name.clone(),
            version: manifest.module.version.clone(),
            language: manifest.module.language.clone(),
            entry: manifest.module.entry.clone(),
            description: manifest.module.description.clone(),
            provides: manifest.module.declare.provides.clone(),
            path: path.to_path_buf(),
            manifest: Some(manifest),
        })
    }

    pub fn entry_path(&self) -> PathBuf {
        self.path.join(&self.entry)
    }
}

impl std::fmt::Display for InstalledModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} v{} [{}] — {}", self.name, self.version, self.language, self.description)
    }
}

// ─── 工具函数 ────────────────────────────────────────

fn copy_dir(src: &Path, dst: &Path) -> Result<(), PackageError> {
    let mut stack = vec![src.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).map_err(|e| PackageError::Io(e))? {
            let entry = entry.map_err(|e| PackageError::Io(e))?;
            let entry_path = entry.path();
            let relative = entry_path.strip_prefix(src)
                .map_err(|e| PackageError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
            let target = dst.join(relative);

            if entry.file_type().map_err(|e| PackageError::Io(e))?.is_dir() {
                fs::create_dir_all(&target).map_err(|e| PackageError::Io(e))?;
                stack.push(entry_path);
            } else {
                fs::copy(&entry_path, &target).map_err(|e| PackageError::Io(e))?;
            }
        }
    }
    Ok(())
}

// ─── 测试 ────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let toml_str = r#"
[module]
id = "emotion.detect"
name = "Emotion Detection"
version = "1.0.0"
language = "python"
entry = "module/main.py"
description = "Detect emotions"

[module.declare]
provides = ["emotion.detect"]
"#;
        let manifest: Manifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.module.id, "emotion.detect");
        assert_eq!(manifest.module.declare.provides, vec!["emotion.detect"]);
    }

    #[test]
    fn test_validate_missing_id() {
        let manifest = Manifest {
            module: ManifestModule {
                id: "".into(),
                name: "Test".into(),
                version: "1.0.0".into(),
                language: "python".into(),
                entry: "main.py".into(),
                description: "".into(),
                author: None,
                declare: ManifestDeclare {
                    provides: vec!["test.service".into()],
                    requires: vec![],
                    handlers: vec![],
                },
            },
            compatibility: None,
            install: None,
            signature: None,
        };
        assert!(PackageReader::validate(&manifest).is_err());
    }
}
