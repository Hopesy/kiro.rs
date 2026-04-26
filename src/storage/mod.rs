use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, bail};
use parking_lot::Mutex;

use crate::kiro::model::credentials::{CredentialsConfig, KiroCredentials};

static GIT_STORAGE: OnceLock<Arc<GitStorage>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct ResolvedPaths {
    pub config_path: PathBuf,
    pub credentials_path: PathBuf,
    pub force_multiple_credentials: bool,
}

#[derive(Debug, Clone)]
struct GitStorageConfig {
    repo_url: String,
    branch: String,
    local_dir: PathBuf,
    config_path: PathBuf,
    credentials_dir: PathBuf,
    author_name: String,
    author_email: String,
}

impl GitStorageConfig {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let repo_url = match env::var("GIT_STORAGE_REPO_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => bail!("GIT_STORAGE_REPO_URL 已设置但为空"),
            Err(env::VarError::NotPresent) => return Ok(None),
            Err(e) => return Err(e).context("读取 GIT_STORAGE_REPO_URL 失败"),
        };

        let local_dir = match env::var("GIT_STORAGE_LOCAL_DIR") {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            Ok(_) => bail!("GIT_STORAGE_LOCAL_DIR 已设置但为空"),
            Err(env::VarError::NotPresent) => PathBuf::from(".git-storage"),
            Err(e) => return Err(e).context("读取 GIT_STORAGE_LOCAL_DIR 失败"),
        };

        let config_path = match env::var("GIT_STORAGE_CONFIG_PATH") {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            Ok(_) => bail!("GIT_STORAGE_CONFIG_PATH 已设置但为空"),
            Err(env::VarError::NotPresent) => PathBuf::from("state/config.json"),
            Err(e) => return Err(e).context("读取 GIT_STORAGE_CONFIG_PATH 失败"),
        };

        let credentials_dir = match env::var("GIT_STORAGE_CREDENTIALS_DIR") {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            Ok(_) => bail!("GIT_STORAGE_CREDENTIALS_DIR 已设置但为空"),
            Err(env::VarError::NotPresent) => PathBuf::from("state/credentials"),
            Err(e) => return Err(e).context("读取 GIT_STORAGE_CREDENTIALS_DIR 失败"),
        };

        Ok(Some(Self {
            repo_url,
            branch: env::var("GIT_STORAGE_BRANCH").unwrap_or_else(|_| "render-state".to_string()),
            local_dir,
            config_path,
            credentials_dir,
            author_name: env::var("GIT_STORAGE_AUTHOR_NAME")
                .unwrap_or_else(|_| "kiro-rs".to_string()),
            author_email: env::var("GIT_STORAGE_AUTHOR_EMAIL")
                .unwrap_or_else(|_| "kiro-rs@localhost".to_string()),
        }))
    }
}

#[derive(Debug)]
struct GitStorage {
    config: GitStorageConfig,
    repo_root: PathBuf,
    runtime_credentials_path: PathBuf,
    repo_config_path: PathBuf,
    repo_credentials_dir: PathBuf,
    lock: Mutex<()>,
}

impl GitStorage {
    fn new(
        config: GitStorageConfig,
        requested_config_path: &Path,
        requested_credentials_path: &Path,
    ) -> anyhow::Result<Self> {
        let repo_root = make_absolute(&config.local_dir)?;
        let runtime_credentials_path = make_absolute(requested_credentials_path)?;
        let repo_config_path = repo_root.join(&config.config_path);
        let repo_credentials_dir = repo_root.join(&config.credentials_dir);

        // requested_config_path 当前仅用于首次导入，本体持久化路径固定到 git worktree 内
        let _ = requested_config_path;

        Ok(Self {
            config,
            repo_root,
            runtime_credentials_path,
            repo_config_path,
            repo_credentials_dir,
            lock: Mutex::new(()),
        })
    }

    fn resolved_paths(&self) -> ResolvedPaths {
        ResolvedPaths {
            config_path: self.repo_config_path.clone(),
            credentials_path: self.runtime_credentials_path.clone(),
            force_multiple_credentials: true,
        }
    }

    fn initialize(
        &self,
        requested_config_path: &Path,
        requested_credentials_path: &Path,
    ) -> anyhow::Result<()> {
        let _guard = self.lock.lock();

        self.ensure_repo_ready()?;

        let mut bootstrap_reason = Vec::new();

        if !self.repo_config_path.exists() && requested_config_path.exists() {
            self.copy_local_config_into_repo(requested_config_path)?;
            bootstrap_reason.push("import config");
        }

        if !self.has_repo_credentials()? && requested_credentials_path.exists() {
            self.import_credentials_file_to_repo(requested_credentials_path)?;
            bootstrap_reason.push("import credentials");
        }

        self.materialize_runtime_credentials()?;

        if !bootstrap_reason.is_empty() {
            self.commit_and_push(&format!("bootstrap: {}", bootstrap_reason.join(", ")))?;
        }

        Ok(())
    }

    fn on_config_saved(&self, path: &Path) -> anyhow::Result<()> {
        if !same_path(path, &self.repo_config_path)? {
            return Ok(());
        }

        let _guard = self.lock.lock();
        self.commit_and_push("persist config")?;
        Ok(())
    }

    fn on_credentials_saved(&self, path: &Path) -> anyhow::Result<()> {
        if !same_path(path, &self.runtime_credentials_path)? {
            return Ok(());
        }

        let _guard = self.lock.lock();
        self.import_credentials_file_to_repo(path)?;
        self.commit_and_push("persist credentials")?;
        Ok(())
    }

    fn ensure_repo_ready(&self) -> anyhow::Result<()> {
        if !self.repo_root.join(".git").exists() {
            if let Some(parent) = self.repo_root.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("创建 git 存储父目录失败: {}", parent.display()))?;
            }

            self.run_git_in(
                None,
                [
                    "clone".to_string(),
                    self.config.repo_url.clone(),
                    self.repo_root.display().to_string(),
                ],
            )
            .context("克隆 git 存储仓库失败")?;
        }

        self.ensure_git_identity()?;
        self.checkout_storage_branch()?;
        Ok(())
    }

    fn ensure_git_identity(&self) -> anyhow::Result<()> {
        self.run_git(["config", "user.name", &self.config.author_name])
            .context("设置 git user.name 失败")?;
        self.run_git(["config", "user.email", &self.config.author_email])
            .context("设置 git user.email 失败")?;
        Ok(())
    }

    fn checkout_storage_branch(&self) -> anyhow::Result<()> {
        let remote_exists = self
            .run_git([
                "ls-remote",
                "--exit-code",
                "--heads",
                "origin",
                &self.config.branch,
            ])
            .is_ok();

        if remote_exists {
            self.run_git(["fetch", "origin", &self.config.branch])
                .with_context(|| format!("抓取远端分支 {} 失败", self.config.branch))?;
            self.run_git(["checkout", "-B", &self.config.branch, "FETCH_HEAD"])
                .with_context(|| format!("切换到远端分支 {} 失败", self.config.branch))?;
        } else {
            self.run_git(["checkout", "-B", &self.config.branch])
                .with_context(|| format!("创建本地分支 {} 失败", self.config.branch))?;
        }

        Ok(())
    }

    fn copy_local_config_into_repo(&self, local_config_path: &Path) -> anyhow::Result<()> {
        let content = fs::read_to_string(local_config_path)
            .with_context(|| format!("读取本地配置失败: {}", local_config_path.display()))?;

        if let Some(parent) = self.repo_config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }

        fs::write(&self.repo_config_path, content)
            .with_context(|| format!("写入 git 配置失败: {}", self.repo_config_path.display()))?;
        Ok(())
    }

    fn has_repo_credentials(&self) -> anyhow::Result<bool> {
        if !self.repo_credentials_dir.exists() {
            return Ok(false);
        }

        for entry in fs::read_dir(&self.repo_credentials_dir)
            .with_context(|| format!("读取凭据目录失败: {}", self.repo_credentials_dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_file() && entry.path().extension() == Some(OsStr::new("json"))
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn load_repo_credentials(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        if !self.repo_credentials_dir.exists() {
            return Ok(vec![]);
        }

        let mut items = Vec::new();
        for entry in fs::read_dir(&self.repo_credentials_dir)
            .with_context(|| format!("读取凭据目录失败: {}", self.repo_credentials_dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.path().extension() != Some(OsStr::new("json"))
            {
                continue;
            }

            let path = entry.path();
            let content = fs::read_to_string(&path)
                .with_context(|| format!("读取凭据文件失败: {}", path.display()))?;
            let mut credential: KiroCredentials = serde_json::from_str(&content)
                .with_context(|| format!("解析凭据文件失败: {}", path.display()))?;
            credential.canonicalize_auth_method();
            items.push((
                path.file_name().map(|s| s.to_string_lossy().to_string()),
                credential,
            ));
        }

        items.sort_by(|(a_name, a), (b_name, b)| a.id.cmp(&b.id).then_with(|| a_name.cmp(b_name)));

        Ok(items
            .into_iter()
            .map(|(_, credential)| credential)
            .collect())
    }

    fn materialize_runtime_credentials(&self) -> anyhow::Result<()> {
        let credentials = self.load_repo_credentials()?;
        let json = serde_json::to_string_pretty(&credentials).context("序列化运行时凭据失败")?;

        if let Some(parent) = self.runtime_credentials_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建运行时凭据目录失败: {}", parent.display()))?;
        }

        fs::write(&self.runtime_credentials_path, json).with_context(|| {
            format!(
                "写入运行时凭据文件失败: {}",
                self.runtime_credentials_path.display()
            )
        })?;
        Ok(())
    }

    fn import_credentials_file_to_repo(&self, path: &Path) -> anyhow::Result<()> {
        let config = CredentialsConfig::load(path)
            .with_context(|| format!("读取运行时凭据失败: {}", path.display()))?;
        let credentials = config.into_sorted_credentials();
        self.write_credentials_to_repo(&credentials)
    }

    fn write_credentials_to_repo(&self, credentials: &[KiroCredentials]) -> anyhow::Result<()> {
        fs::create_dir_all(&self.repo_credentials_dir).with_context(|| {
            format!(
                "创建 git 凭据目录失败: {}",
                self.repo_credentials_dir.display()
            )
        })?;

        let mut expected = Vec::new();
        for (index, credential) in credentials.iter().enumerate() {
            let file_name = credential_file_name(credential, index);
            let path = self.repo_credentials_dir.join(&file_name);
            let mut data = credential.clone();
            data.canonicalize_auth_method();
            let json = serde_json::to_string_pretty(&data)
                .with_context(|| format!("序列化凭据失败: {}", file_name))?;
            fs::write(&path, json)
                .with_context(|| format!("写入 git 凭据失败: {}", path.display()))?;
            expected.push(file_name);
        }

        for entry in fs::read_dir(&self.repo_credentials_dir).with_context(|| {
            format!(
                "读取 git 凭据目录失败: {}",
                self.repo_credentials_dir.display()
            )
        })? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            if entry.path().extension() == Some(OsStr::new("json"))
                && !expected.contains(&file_name)
            {
                fs::remove_file(entry.path())
                    .with_context(|| format!("删除旧凭据文件失败: {}", file_name))?;
            }
        }

        Ok(())
    }

    fn commit_and_push(&self, message: &str) -> anyhow::Result<()> {
        self.run_git(["add", "--all", "."])
            .context("git add 失败")?;

        if self
            .run_git(["diff", "--cached", "--quiet"])
            .map(|_| true)
            .unwrap_or(false)
        {
            return Ok(());
        }

        self.run_git(["commit", "-m", message])
            .with_context(|| format!("git commit 失败: {}", message))?;

        if self
            .run_git(["push", "-u", "origin", &self.config.branch])
            .is_err()
        {
            let _ = self.run_git(["pull", "--rebase", "origin", &self.config.branch]);
            self.run_git(["push", "-u", "origin", &self.config.branch])
                .with_context(|| format!("git push 失败: {}", self.config.branch))?;
        }

        Ok(())
    }

    fn run_git<'a, I, S>(&self, args: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.run_git_in(Some(&self.repo_root), args)
    }

    fn run_git_in<'a, I, S>(&self, workdir: Option<&Path>, args: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args_vec: Vec<String> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect();

        let mut command = Command::new("git");
        command.args(&args_vec);
        if let Some(workdir) = workdir {
            command.current_dir(workdir);
        }
        command.env("GIT_TERMINAL_PROMPT", "0");

        let output = command
            .output()
            .with_context(|| format!("执行 git 命令失败: git {}", args_vec.join(" ")))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };

        bail!(
            "git {} 失败{}",
            args_vec.join(" "),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", detail)
            }
        )
    }
}

fn credential_file_name(credential: &KiroCredentials, index: usize) -> String {
    match credential.id {
        Some(id) => format!("credential-{:06}.json", id),
        None => format!("credential-new-{:06}.json", index + 1),
    }
}

fn make_absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(env::current_dir()
        .context("读取当前工作目录失败")?
        .join(path))
}

fn same_path(left: &Path, right: &Path) -> anyhow::Result<bool> {
    Ok(make_absolute(left)? == make_absolute(right)?)
}

pub fn initialize_from_env(
    requested_config_path: &Path,
    requested_credentials_path: &Path,
) -> anyhow::Result<Option<ResolvedPaths>> {
    let Some(config) = GitStorageConfig::from_env()? else {
        return Ok(None);
    };

    let storage = Arc::new(GitStorage::new(
        config,
        requested_config_path,
        requested_credentials_path,
    )?);
    storage.initialize(requested_config_path, requested_credentials_path)?;

    let resolved = storage.resolved_paths();

    if let Some(existing) = GIT_STORAGE.get() {
        if existing.resolved_paths().config_path != resolved.config_path
            || existing.resolved_paths().credentials_path != resolved.credentials_path
        {
            bail!("Git 外部存储已初始化为不同路径，拒绝重复初始化");
        }
    } else {
        let _ = GIT_STORAGE.set(storage);
    }

    Ok(Some(resolved))
}

pub fn notify_config_written(path: &Path) -> anyhow::Result<()> {
    if let Some(storage) = GIT_STORAGE.get() {
        storage.on_config_saved(path)?;
    }
    Ok(())
}

pub fn notify_credentials_written(path: &Path) -> anyhow::Result<()> {
    if let Some(storage) = GIT_STORAGE.get() {
        storage.on_credentials_saved(path)?;
    }
    Ok(())
}
