use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, bail};
use parking_lot::Mutex;

use crate::kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use crate::model::config::Config;

static STORAGE_BACKEND: OnceLock<StorageBackend> = OnceLock::new();
const GIT_STORAGE_BRANCH: &str = "main";
const GIT_STORAGE_LOCAL_DIR: &str = ".git-storage";
const GIT_STORAGE_CONFIG_PATH: &str = "config/config.json";
const GIT_STORAGE_CREDENTIALS_DIR: &str = "auths";

#[derive(Debug, Clone)]
pub struct ResolvedPaths {
    pub config_path: PathBuf,
    pub credentials_path: PathBuf,
    pub force_multiple_credentials: bool,
}

#[derive(Debug, Clone)]
struct GitStorageConfig {
    repo_url: String,
    auth_token: Option<String>,
    branch: String,
    local_dir: PathBuf,
    config_path: PathBuf,
    credentials_dir: PathBuf,
}

enum StorageBackend {
    Git(Arc<GitStorage>),
    Local(Arc<LocalStorage>),
}

impl StorageBackend {
    fn resolved_paths(&self) -> ResolvedPaths {
        match self {
            StorageBackend::Git(storage) => storage.resolved_paths(),
            StorageBackend::Local(storage) => storage.resolved_paths(),
        }
    }

    fn on_config_saved(&self, path: &Path) -> anyhow::Result<()> {
        match self {
            StorageBackend::Git(storage) => storage.on_config_saved(path),
            StorageBackend::Local(storage) => storage.on_config_saved(path),
        }
    }

    fn on_credentials_saved(&self, path: &Path) -> anyhow::Result<()> {
        match self {
            StorageBackend::Git(storage) => storage.on_credentials_saved(path),
            StorageBackend::Local(storage) => storage.on_credentials_saved(path),
        }
    }
}

impl GitStorageConfig {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let repo_url = match env::var("GIT_STORAGE_REPO_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => bail!("GIT_STORAGE_REPO_URL 已设置但为空"),
            Err(env::VarError::NotPresent) => return Ok(None),
            Err(e) => return Err(e).context("读取 GIT_STORAGE_REPO_URL 失败"),
        };

        Ok(Some(Self {
            repo_url,
            auth_token: match env::var("GIT_STORAGE_AUTH_TOKEN") {
                Ok(value) if !value.trim().is_empty() => Some(value),
                Ok(_) => bail!("GIT_STORAGE_AUTH_TOKEN 已设置但为空"),
                Err(env::VarError::NotPresent) => None,
                Err(e) => return Err(e).context("读取 GIT_STORAGE_AUTH_TOKEN 失败"),
            },
            branch: GIT_STORAGE_BRANCH.to_string(),
            local_dir: PathBuf::from(GIT_STORAGE_LOCAL_DIR),
            config_path: PathBuf::from(GIT_STORAGE_CONFIG_PATH),
            credentials_dir: PathBuf::from(GIT_STORAGE_CREDENTIALS_DIR),
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

#[derive(Debug)]
struct LocalStorage {
    runtime_credentials_path: PathBuf,
    config_path: PathBuf,
    credentials_dir: PathBuf,
}

impl LocalStorage {
    fn new(
        _requested_config_path: &Path,
        _requested_credentials_path: &Path,
    ) -> anyhow::Result<Self> {
        let data_root = default_local_data_root()?;
        Ok(Self {
            runtime_credentials_path: data_root.join("runtime").join("credentials.json"),
            config_path: data_root.join("config").join("config.json"),
            credentials_dir: data_root.join("auths"),
        })
    }

    fn resolved_paths(&self) -> ResolvedPaths {
        ResolvedPaths {
            config_path: self.config_path.clone(),
            credentials_path: self.runtime_credentials_path.clone(),
            force_multiple_credentials: true,
        }
    }

    fn initialize(
        &self,
        requested_config_path: &Path,
        requested_credentials_path: &Path,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(self.config_path.parent().expect("config parent exists"))
            .with_context(|| format!("创建本地配置目录失败: {}", self.config_path.display()))?;
        fs::create_dir_all(&self.credentials_dir)
            .with_context(|| format!("创建本地账号目录失败: {}", self.credentials_dir.display()))?;
        fs::create_dir_all(
            self.runtime_credentials_path
                .parent()
                .expect("runtime parent exists"),
        )
        .with_context(|| {
            format!(
                "创建本地运行时目录失败: {}",
                self.runtime_credentials_path.display()
            )
        })?;

        if !self.config_path.exists() {
            if requested_config_path.exists() {
                fs::copy(requested_config_path, &self.config_path).with_context(|| {
                    format!(
                        "导入本地配置失败: {} -> {}",
                        requested_config_path.display(),
                        self.config_path.display()
                    )
                })?;
            } else {
                write_config_file(&self.config_path, &Config::bootstrap_from_env()).with_context(
                    || format!("写入本地初始配置失败: {}", self.config_path.display()),
                )?;
            }
        }

        if !self.has_local_credentials()? && requested_credentials_path.exists() {
            self.import_credentials_file_to_dir(requested_credentials_path)?;
        }

        self.materialize_runtime_credentials()?;
        Ok(())
    }

    fn has_local_credentials(&self) -> anyhow::Result<bool> {
        if !self.credentials_dir.exists() {
            return Ok(false);
        }

        for entry in fs::read_dir(&self.credentials_dir)
            .with_context(|| format!("读取本地账号目录失败: {}", self.credentials_dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_file() && entry.path().extension() == Some(OsStr::new("json"))
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn load_local_credentials(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        load_credentials_from_dir(&self.credentials_dir)
    }

    fn materialize_runtime_credentials(&self) -> anyhow::Result<()> {
        let credentials = self.load_local_credentials()?;
        let json =
            serde_json::to_string_pretty(&credentials).context("序列化本地运行时凭据失败")?;
        fs::write(&self.runtime_credentials_path, json).with_context(|| {
            format!(
                "写入本地运行时凭据失败: {}",
                self.runtime_credentials_path.display()
            )
        })?;
        Ok(())
    }

    fn import_credentials_file_to_dir(&self, path: &Path) -> anyhow::Result<()> {
        let config = CredentialsConfig::load(path)
            .with_context(|| format!("读取本地凭据失败: {}", path.display()))?;
        let credentials = config.into_sorted_credentials();
        write_credentials_to_dir(&self.credentials_dir, &credentials)
    }

    fn on_config_saved(&self, path: &Path) -> anyhow::Result<()> {
        if !same_path(path, &self.config_path)? {
            return Ok(());
        }

        tracing::debug!("本地配置已持久化到数据目录: {}", self.config_path.display());
        Ok(())
    }

    fn on_credentials_saved(&self, path: &Path) -> anyhow::Result<()> {
        if !same_path(path, &self.runtime_credentials_path)? {
            return Ok(());
        }

        self.import_credentials_file_to_dir(path)?;
        tracing::debug!(
            "本地账号已同步到数据目录: {}",
            self.credentials_dir.display()
        );
        Ok(())
    }
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

        if !self.repo_config_path.exists() {
            self.bootstrap_config_into_repo()?;
            bootstrap_reason.push("bootstrap config");
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
                    self.authenticated_repo_url()?,
                    self.repo_root.display().to_string(),
                ],
            )
            .context("克隆 git 存储仓库失败")?;
        }

        self.ensure_git_identity()?;
        self.run_git([
            "remote",
            "set-url",
            "origin",
            &self.authenticated_repo_url()?,
        ])
        .context("更新 origin 远端地址失败")?;
        self.checkout_storage_branch()?;
        Ok(())
    }

    fn authenticated_repo_url(&self) -> anyhow::Result<String> {
        let Some(token) = &self.config.auth_token else {
            return Ok(self.config.repo_url.clone());
        };

        let mut url = reqwest::Url::parse(&self.config.repo_url)
            .with_context(|| format!("解析 GIT_STORAGE_REPO_URL 失败: {}", self.config.repo_url))?;
        url.set_username("x-access-token")
            .map_err(|_| anyhow::anyhow!("为 git 仓库 URL 设置用户名失败"))?;
        url.set_password(Some(token))
            .map_err(|_| anyhow::anyhow!("为 git 仓库 URL 设置密码失败"))?;
        Ok(url.to_string())
    }

    fn ensure_git_identity(&self) -> anyhow::Result<()> {
        self.run_git(["config", "user.name", "kiro-rs"])
            .context("设置 git user.name 失败")?;
        self.run_git(["config", "user.email", "kiro-rs@localhost"])
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

    fn bootstrap_config_into_repo(&self) -> anyhow::Result<()> {
        let config = Config::bootstrap_from_env();
        if config.api_key.is_none() {
            bail!(
                "git 数据仓库中缺少配置文件: {}；首次启动请提供 PUBLIC_API_KEY（可选 ADMIN_API_KEY、KIRO_REGION）或预先提交 config/config.json",
                self.repo_config_path.display()
            );
        }

        write_config_file(&self.repo_config_path, &config)
            .with_context(|| format!("写入 git 初始配置失败: {}", self.repo_config_path.display()))
    }

    fn has_repo_credentials(&self) -> anyhow::Result<bool> {
        has_credentials_in_dir(&self.repo_credentials_dir)
    }

    fn load_repo_credentials(&self) -> anyhow::Result<Vec<KiroCredentials>> {
        load_credentials_from_dir(&self.repo_credentials_dir)
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
        write_credentials_to_dir(&self.repo_credentials_dir, &credentials)
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

fn has_credentials_in_dir(dir: &Path) -> anyhow::Result<bool> {
    if !dir.exists() {
        return Ok(false);
    }

    for entry in
        fs::read_dir(dir).with_context(|| format!("读取账号目录失败: {}", dir.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension() == Some(OsStr::new("json")) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn load_credentials_from_dir(dir: &Path) -> anyhow::Result<Vec<KiroCredentials>> {
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut items = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("读取账号目录失败: {}", dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.path().extension() != Some(OsStr::new("json")) {
            continue;
        }

        let path = entry.path();
        let content = fs::read_to_string(&path)
            .with_context(|| format!("读取账号文件失败: {}", path.display()))?;
        let mut credential: KiroCredentials = serde_json::from_str(&content)
            .with_context(|| format!("解析账号文件失败: {}", path.display()))?;
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

fn write_credentials_to_dir(dir: &Path, credentials: &[KiroCredentials]) -> anyhow::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("创建账号目录失败: {}", dir.display()))?;

    let mut expected = Vec::new();
    for (index, credential) in credentials.iter().enumerate() {
        let file_name = credential_file_name(credential, index);
        let path = dir.join(&file_name);
        let mut data = credential.clone();
        data.canonicalize_auth_method();
        let json = serde_json::to_string_pretty(&data)
            .with_context(|| format!("序列化账号失败: {}", file_name))?;
        fs::write(&path, json).with_context(|| format!("写入账号文件失败: {}", path.display()))?;
        expected.push(file_name);
    }

    for entry in
        fs::read_dir(dir).with_context(|| format!("读取账号目录失败: {}", dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if entry.path().extension() == Some(OsStr::new("json")) && !expected.contains(&file_name) {
            fs::remove_file(entry.path())
                .with_context(|| format!("删除旧账号文件失败: {}", file_name))?;
        }
    }

    Ok(())
}

fn write_config_file(path: &Path, config: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(config).context("序列化配置失败")?;
    fs::write(path, content).with_context(|| format!("写入配置文件失败: {}", path.display()))?;
    Ok(())
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

fn default_local_data_root() -> anyhow::Result<PathBuf> {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        if !local_app_data.trim().is_empty() {
            return Ok(PathBuf::from(local_app_data).join("kiro-rs"));
        }
    }

    if let Ok(xdg_data_home) = env::var("XDG_DATA_HOME") {
        if !xdg_data_home.trim().is_empty() {
            return Ok(PathBuf::from(xdg_data_home).join("kiro-rs"));
        }
    }

    if let Ok(home) = env::var("HOME") {
        if !home.trim().is_empty() {
            return Ok(PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("kiro-rs"));
        }
    }

    bail!("无法确定本地数据目录：LOCALAPPDATA/XDG_DATA_HOME/HOME 均不可用")
}

pub fn initialize(
    requested_config_path: &Path,
    requested_credentials_path: &Path,
    allow_local_directory_defaults: bool,
) -> anyhow::Result<Option<ResolvedPaths>> {
    let backend = if let Some(config) = GitStorageConfig::from_env()? {
        let storage = Arc::new(GitStorage::new(
            config,
            requested_config_path,
            requested_credentials_path,
        )?);
        storage.initialize(requested_config_path, requested_credentials_path)?;
        Some(StorageBackend::Git(storage))
    } else if allow_local_directory_defaults {
        let storage = Arc::new(LocalStorage::new(
            requested_config_path,
            requested_credentials_path,
        )?);
        storage.initialize(requested_config_path, requested_credentials_path)?;
        Some(StorageBackend::Local(storage))
    } else {
        None
    };

    let Some(backend) = backend else {
        return Ok(None);
    };
    let resolved = backend.resolved_paths();

    if let Some(existing) = STORAGE_BACKEND.get() {
        if existing.resolved_paths().config_path != resolved.config_path
            || existing.resolved_paths().credentials_path != resolved.credentials_path
        {
            bail!("存储后端已初始化为不同路径，拒绝重复初始化");
        }
    } else {
        let _ = STORAGE_BACKEND.set(backend);
    }

    Ok(Some(resolved))
}

pub fn notify_config_written(path: &Path) -> anyhow::Result<()> {
    if let Some(storage) = STORAGE_BACKEND.get() {
        storage.on_config_saved(path)?;
    }
    Ok(())
}

pub fn notify_credentials_written(path: &Path) -> anyhow::Result<()> {
    if let Some(storage) = STORAGE_BACKEND.get() {
        storage.on_credentials_saved(path)?;
    }
    Ok(())
}
