use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use crate::{Header, HttpRequest, HttpResponse, HttpVersion};
use mio_proxy::SendConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_RELEASES_API_URL: &str =
    "https://api.github.com/repos/ltlly/MIO/releases/latest";
pub const ENV_UPDATE_API_URL: &str = "MIO_UPDATE_API_URL";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const CHECKSUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
const MAX_REDIRECTS: usize = 5;
const API_MAX_BYTES: usize = 2 * 1024 * 1024;
const ASSET_MAX_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub tag_name: String,
    pub release_notes: String,
    pub html_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub checksums_url: Option<String>,
    pub asset_sha256: Option<String>,
    pub asset_size: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate {
        current_version: String,
    },
    Available(UpdateInfo),
    Downloading {
        version: String,
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    Verifying {
        version: String,
    },
    Installing {
        version: String,
    },
    RestartRequired {
        version: String,
        relaunched: bool,
    },
    Error(String),
}

#[derive(Deserialize)]
struct GitHubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PreId {
    Num(u64),
    Text(String),
}

impl Ord for PreId {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Num(a), Self::Num(b)) => a.cmp(b),
            (Self::Num(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Num(_)) => Ordering::Greater,
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PreId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_semver(raw: &str) -> Result<SemVer> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let without_build = stripped.split('+').next().unwrap_or(stripped);
    let (core, pre_str) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    let major = parts
        .next()
        .context("版本号缺少主版本号")?
        .parse::<u64>()
        .context("主版本号无效")?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().context("次版本号无效")?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().context("修订号无效")?;
    ensure!(parts.next().is_none(), "版本号包含过多段：{raw}");
    let pre = match pre_str {
        Some(p) if !p.is_empty() => p
            .split('.')
            .map(|seg| match seg.parse::<u64>() {
                Ok(num) => PreId::Num(num),
                Err(_) => PreId::Text(seg.to_owned()),
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(SemVer {
        major,
        minor,
        patch,
        pre,
    })
}

/// Returns `true` when `candidate` is strictly newer than `current` per SemVer.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    let (Ok(cur), Ok(cand)) = (parse_semver(current), parse_semver(candidate)) else {
        return false;
    };
    cand > cur
}

pub fn normalized_version(raw: &str) -> String {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
        .to_owned()
}

pub fn resolved_api_url() -> String {
    std::env::var(ENV_UPDATE_API_URL)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASES_API_URL.to_owned())
}

pub fn platform_asset_suffixes_for(os: &str, arch: &str) -> Vec<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") | ("macos", "arm64") => vec!["-macos-arm64.zip", "-macos-aarch64.zip"],
        ("macos", "x86_64") | ("macos", "amd64") => vec!["-macos-x86_64.zip"],
        ("linux", "aarch64") | ("linux", "arm64") => {
            vec!["-linux-aarch64.tar.gz", "-linux-arm64.tar.gz"]
        }
        ("linux", "x86_64") | ("linux", "amd64") => vec!["-linux-x86_64.tar.gz"],
        ("windows", "aarch64") | ("windows", "arm64") => {
            vec!["-windows-aarch64.zip", "-windows-arm64.zip"]
        }
        ("windows", "x86_64") | ("windows", "amd64") => vec!["-windows-x86_64.zip"],
        _ => Vec::new(),
    }
}

pub fn current_platform_asset_suffixes() -> Vec<&'static str> {
    platform_asset_suffixes_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Parses a `SHA256SUMS.txt` file (`<hex>  <filename>`) for `target_asset`.
pub fn parse_sha256_sums(sums_text: &str, target_asset: &str) -> Option<String> {
    let target_base = target_asset.rsplit(['/', '\\']).next().unwrap_or(target_asset);
    for line in sums_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name_raw) = parts.next() else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let cleaned = name_raw.trim_start_matches('*');
        let base = cleaned.rsplit(['/', '\\']).next().unwrap_or(cleaned);
        if base.eq_ignore_ascii_case(target_base) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn header_value(resp: &HttpResponse, name: &str) -> Option<String> {
    resp.headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .and_then(|h| std::str::from_utf8(&h.value).ok())
        .map(|s| s.trim().to_owned())
}

async fn http_get_bytes(
    initial_url: &str,
    accept: &str,
    timeout: Duration,
    max_body_bytes: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    let mut current_url = initial_url.to_owned();
    for _hop in 0..=MAX_REDIRECTS {
        if cancel.is_cancelled() {
            bail!("更新操作已取消");
        }
        let mut headers = vec![
            Header {
                name: "User-Agent".into(),
                value: format!("MIO/{CURRENT_VERSION}").into_bytes(),
            },
            Header {
                name: "Accept".into(),
                value: accept.as_bytes().to_vec(),
            },
        ];
        if let Ok(parsed) = url::Url::parse(&current_url)
            && let Some(host) = parsed.host_str()
        {
            let host_hdr = match parsed.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            };
            headers.push(Header {
                name: "Host".into(),
                value: host_hdr.into_bytes(),
            });
        }
        let req = HttpRequest {
            method: "GET".into(),
            url: current_url.clone(),
            version: HttpVersion::Http11,
            headers,
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let cfg = SendConfig {
            timeout,
            max_body_bytes,
            use_connection_pool: false,
            ..SendConfig::default()
        };
        let resp = mio_proxy::send(req, cfg, cancel.clone())
            .await
            .map_err(|e| anyhow!("请求更新服务器失败：{e}"))?;
        if matches!(resp.status, 301 | 302 | 303 | 307 | 308) {
            let loc = header_value(&resp, "location")
                .context("更新服务器返回重定向响应，但缺少 Location 响应头")?;
            let next = url::Url::parse(&current_url)
                .with_context(|| format!("无效的请求 URL：{current_url}"))?
                .join(&loc)
                .with_context(|| format!("无效的重定向地址：{loc}"))?;
            current_url = next.to_string();
            continue;
        }
        if !(200..300).contains(&resp.status) {
            let snippet = String::from_utf8_lossy(&resp.body[..resp.body.len().min(200)]);
            bail!(
                "更新服务器返回 HTTP {}：{}",
                resp.status,
                snippet.trim()
            );
        }
        return Ok(resp.body);
    }
    bail!("更新请求重定向次数过多（超过 {MAX_REDIRECTS} 次）")
}

/// Parses a GitHub Releases JSON payload into either `UpToDate` or `Available(UpdateInfo)`.
pub fn parse_release_response(
    json_bytes: &[u8],
    current_version: &str,
    asset_suffixes: &[&str],
) -> Result<UpdateStatus> {
    let release: GitHubRelease =
        serde_json::from_slice(json_bytes).context("无法解析更新元数据 JSON")?;
    if release.draft || release.tag_name.trim().is_empty() {
        return Ok(UpdateStatus::UpToDate {
            current_version: current_version.to_owned(),
        });
    }
    let latest_version = normalized_version(&release.tag_name);
    if !is_newer_version(current_version, &latest_version) {
        return Ok(UpdateStatus::UpToDate {
            current_version: current_version.to_owned(),
        });
    }
    let asset = asset_suffixes
        .iter()
        .find_map(|suffix| {
            release
                .assets
                .iter()
                .find(|a| a.name.to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase()))
        })
        .with_context(|| {
            format!(
                "最新 Release {} 未包含当前平台 ({}/{}) 的安装包",
                release.tag_name,
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
    let checksums_url = release
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(CHECKSUMS_ASSET_NAME))
        .map(|a| a.browser_download_url.clone());

    Ok(UpdateStatus::Available(UpdateInfo {
        current_version: current_version.to_owned(),
        latest_version,
        tag_name: release.tag_name,
        release_notes: release.body.unwrap_or_default(),
        html_url: release.html_url,
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        checksums_url,
        asset_sha256: None,
        asset_size: asset.size,
    }))
}

/// Queries the GitHub Releases API (or `api_url`) and reports update status.
pub async fn check_for_updates(
    api_url: &str,
    current_version: &str,
    cancel: &CancellationToken,
) -> UpdateStatus {
    let suffixes = current_platform_asset_suffixes();
    if suffixes.is_empty() {
        return UpdateStatus::Error(format!(
            "当前平台 ({}/{}) 暂不支持自动更新",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    let json_bytes = match http_get_bytes(
        api_url,
        "application/vnd.github+json",
        Duration::from_secs(20),
        API_MAX_BYTES,
        cancel,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(err) => return UpdateStatus::Error(err.to_string()),
    };
    let mut status = match parse_release_response(&json_bytes, current_version, &suffixes) {
        Ok(status) => status,
        Err(err) => return UpdateStatus::Error(err.to_string()),
    };
    if let UpdateStatus::Available(ref mut info) = status
        && let Some(sums_url) = info.checksums_url.clone()
        && let Ok(sums_bytes) = http_get_bytes(
            &sums_url,
            "text/plain, */*",
            Duration::from_secs(15),
            API_MAX_BYTES,
            cancel,
        )
        .await
    {
        let sums_text = String::from_utf8_lossy(&sums_bytes);
        info.asset_sha256 = parse_sha256_sums(&sums_text, &info.asset_name);
    }
    status
}

/// Verifies `archive_bytes` against either cached hash or downloaded `SHA256SUMS.txt`.
pub async fn verify_archive_checksum(
    info: &UpdateInfo,
    archive_bytes: &[u8],
    cancel: &CancellationToken,
) -> Result<String> {
    let expected = if let Some(expected) = info.asset_sha256.clone() {
        expected
    } else if let Some(sums_url) = &info.checksums_url {
        let sums_bytes = http_get_bytes(
            sums_url,
            "text/plain, */*",
            Duration::from_secs(20),
            API_MAX_BYTES,
            cancel,
        )
        .await
        .context("下载 SHA256SUMS.txt 校验文件失败")?;
        let sums_text = String::from_utf8_lossy(&sums_bytes);
        parse_sha256_sums(&sums_text, &info.asset_name).with_context(|| {
            format!("SHA256SUMS.txt 中缺少 {} 的校验和", info.asset_name)
        })?
    } else {
        bail!("Release 缺少 SHA256SUMS.txt 校验文件，已拒绝安装未校验的更新包");
    };
    let actual = sha256_hex(archive_bytes);
    ensure!(
        actual.eq_ignore_ascii_case(&expected),
        "安装包 SHA-256 校验失败：期望 {expected}，实际 {actual}"
    );
    Ok(actual)
}

/// Downloads, verifies SHA-256, extracts, swaps, and schedules relaunch.
pub async fn download_and_install_update<F>(
    info: &UpdateInfo,
    mut on_status: F,
    cancel: &CancellationToken,
) -> UpdateStatus
where
    F: FnMut(UpdateStatus) + Send,
{
    let version = info.latest_version.clone();
    on_status(UpdateStatus::Downloading {
        version: version.clone(),
        received_bytes: 0,
        total_bytes: (info.asset_size > 0).then_some(info.asset_size),
    });

    let bytes = match http_get_bytes(
        &info.asset_url,
        "application/octet-stream, */*",
        Duration::from_secs(300),
        ASSET_MAX_BYTES,
        cancel,
    )
    .await
    {
        Ok(b) => b,
        Err(err) => return UpdateStatus::Error(err.to_string()),
    };

    on_status(UpdateStatus::Downloading {
        version: version.clone(),
        received_bytes: bytes.len() as u64,
        total_bytes: Some(bytes.len() as u64),
    });
    on_status(UpdateStatus::Verifying {
        version: version.clone(),
    });

    if let Err(err) = verify_archive_checksum(info, &bytes, cancel).await {
        return UpdateStatus::Error(err.to_string());
    }

    on_status(UpdateStatus::Installing {
        version: version.clone(),
    });

    let install_result = tokio::task::spawn_blocking(move || install_archive_and_relaunch(&bytes))
        .await
        .map_err(|e| anyhow!("安装更新任务异常中止：{e}"))
        .and_then(|res| res);

    match install_result {
        Ok(relaunched) => UpdateStatus::RestartRequired {
            version,
            relaunched,
        },
        Err(err) => UpdateStatus::Error(err.to_string()),
    }
}

fn find_named_entry_recursive(root: &Path, name: &str, want_dir: bool) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let is_dir = path.is_dir();
        if file_name.to_str().is_some_and(|s| s == name) && is_dir == want_dir {
            return Some(path);
        }
        if is_dir
            && let Some(found) = find_named_entry_recursive(&path, name, want_dir)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn resolve_macos_app_bundle(exe_path: &Path) -> Option<PathBuf> {
    for ancestor in exe_path.ancestors() {
        if ancestor.extension().is_some_and(|ext| ext == "app") && ancestor.is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn install_archive_and_relaunch(bytes: &[u8]) -> Result<bool> {
    let current_exe = std::env::current_exe().context("无法定位当前可执行文件路径")?;
    let staging = tempfile::Builder::new()
        .prefix("mio-update-")
        .tempdir()
        .context("无法创建更新临时目录")?
        .keep();
    let zip_path = staging.join("update.zip");
    let extract_dir = staging.join("extracted");
    fs::create_dir_all(&extract_dir)?;
    fs::write(&zip_path, bytes)?;

    let status = std::process::Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(&zip_path)
        .arg(&extract_dir)
        .status()
        .context("无法执行 /usr/bin/ditto 解压更新包")?;
    ensure!(status.success(), "/usr/bin/ditto 解压失败：{status}");

    let new_app = find_named_entry_recursive(&extract_dir, "MIO.app", true)
        .context("更新包中缺少 MIO.app")?;
    let new_exe = new_app.join("Contents/MacOS/mio");
    ensure!(new_exe.is_file(), "更新包中的 MIO.app 缺少可执行文件 Contents/MacOS/mio");

    let (src, dst, relaunch, use_open) = if let Some(bundle) = resolve_macos_app_bundle(&current_exe) {
        let relaunch = bundle.clone();
        (new_app, bundle, relaunch, "1")
    } else {
        (new_exe, current_exe.clone(), current_exe, "0")
    };
    let bak = staging.join("previous-backup");
    let pid = std::process::id().to_string();
    let script = r#"
pid="$1"; src="$2"; dst="$3"; bak="$4"; relaunch="$5"; use_open="$6"; stage="$7"
i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 120 ]; do
  sleep 0.25
  i=$((i + 1))
done
rm -rf "$bak"
if [ -e "$dst" ]; then
  mv "$dst" "$bak" || exit 1
fi
if ! mv "$src" "$dst"; then
  [ -e "$bak" ] && mv "$bak" "$dst"
  exit 1
fi
rm -rf "$stage"
if [ "$use_open" = "1" ]; then
  open -n "$relaunch"
else
  nohup "$relaunch" >/dev/null 2>&1 &
fi
"#;
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("mio-self-update")
        .arg(&pid)
        .arg(&src)
        .arg(&dst)
        .arg(&bak)
        .arg(&relaunch)
        .arg(use_open)
        .arg(&staging)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("无法启动 macOS 自替换升级脚本")?;
    Ok(true)
}

#[cfg(target_os = "linux")]
fn install_archive_and_relaunch(bytes: &[u8]) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt as _;

    let current_exe = std::env::current_exe().context("无法定位当前可执行文件路径")?;
    let staging = tempfile::Builder::new()
        .prefix("mio-update-")
        .tempdir()
        .context("无法创建更新临时目录")?;
    let archive_path = staging.path().join("update.tar.gz");
    let extract_dir = staging.path().join("extracted");
    fs::create_dir_all(&extract_dir)?;
    fs::write(&archive_path, bytes)?;

    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .context("无法执行 tar 解压更新包")?;
    ensure!(status.success(), "tar 解压更新包失败：{status}");

    let new_bin = extract_dir
        .join("bin/mio")
        .is_file()
        .then(|| extract_dir.join("bin/mio"))
        .or_else(|| find_named_entry_recursive(&extract_dir, "mio", false))
        .context("更新包中缺少 mio 可执行文件")?;

    let mut perms = fs::metadata(&new_bin)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&new_bin, perms)?;

    let parent = current_exe.parent().context("无法确定可执行文件目录")?;
    let staged_new = parent.join(".mio-update-new");
    let staged_old = parent.join(".mio-update-old");
    let _ = fs::remove_file(&staged_new);
    let _ = fs::remove_file(&staged_old);

    fs::copy(&new_bin, &staged_new).context("写入新版二进制失败，请确认安装目录具有写权限")?;
    let mut dst_perms = fs::metadata(&staged_new)?.permissions();
    dst_perms.set_mode(0o755);
    fs::set_permissions(&staged_new, dst_perms)?;

    fs::rename(&current_exe, &staged_old).context("备份旧版二进制失败")?;
    if let Err(err) = fs::rename(&staged_new, &current_exe) {
        let _ = fs::rename(&staged_old, &current_exe);
        return Err(err).context("替换新版本二进制失败");
    }
    let _ = fs::remove_file(&staged_old);

    std::process::Command::new(&current_exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("无法重启更新后的 MIO 进程")?;
    Ok(true)
}

#[cfg(target_os = "windows")]
fn install_archive_and_relaunch(bytes: &[u8]) -> Result<bool> {
    let current_exe = std::env::current_exe().context("无法定位当前可执行文件路径")?;
    let staging = tempfile::Builder::new()
        .prefix("mio-update-")
        .tempdir()
        .context("无法创建更新临时目录")?
        .keep();
    let zip_path = staging.join("update.zip");
    let extract_dir = staging.join("extracted");
    fs::create_dir_all(&extract_dir)?;
    fs::write(&zip_path, bytes)?;

    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Expand-Archive -LiteralPath $args[0] -DestinationPath $args[1] -Force",
        ])
        .arg(&zip_path)
        .arg(&extract_dir)
        .status()
        .context("无法调用 PowerShell 解压更新包")?;
    ensure!(status.success(), "PowerShell 解压失败：{status}");

    let new_exe = find_named_entry_recursive(&extract_dir, "mio.exe", false)
        .context("更新包中缺少 mio.exe")?;
    let bak = staging.join("mio.exe.old");
    let ps_script = staging.join("apply-update.ps1");
    fs::write(
        &ps_script,
        r#"param([int]$OldPid, [string]$Src, [string]$Dst, [string]$Bak, [string]$Stage)
for ($i = 0; $i -lt 120; $i++) {
    if (-not (Get-Process -Id $OldPid -ErrorAction SilentlyContinue)) { break }
    Start-Sleep -Milliseconds 250
}
try {
    if (Test-Path -LiteralPath $Dst) {
        Move-Item -LiteralPath $Dst -Destination $Bak -Force
    }
    Move-Item -LiteralPath $Src -Destination $Dst -Force
    Start-Process -FilePath $Dst | Out-Null
} catch {
    if ((Test-Path -LiteralPath $Bak) -and -not (Test-Path -LiteralPath $Dst)) {
        Move-Item -LiteralPath $Bak -Destination $Dst -Force
    }
    throw
} finally {
    Remove-Item -LiteralPath $Stage -Recurse -Force -ErrorAction SilentlyContinue
}
"#,
    )?;
    let pid = std::process::id().to_string();
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-File"])
        .arg(&ps_script)
        .args([
            "-OldPid",
            &pid,
            "-Src",
            &new_exe.to_string_lossy(),
            "-Dst",
            &current_exe.to_string_lossy(),
            "-Bak",
            &bak.to_string_lossy(),
            "-Stage",
            &staging.to_string_lossy(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("无法启动 Windows 自替换升级脚本")?;
    Ok(true)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn install_archive_and_relaunch(_bytes: &[u8]) -> Result<bool> {
    bail!("当前操作系统暂不支持自动安装更新")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn semver_and_checksums_parsing_behave_accurately() {
        assert!(is_newer_version("0.2.0", "v0.2.1"));
        assert!(is_newer_version("0.2.0", "0.3.0"));
        assert!(is_newer_version("0.2.0-rc.1", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.3.0", "v0.2.9"));
        assert!(!is_newer_version("0.2.0", "0.2.0-beta.1"));

        let sums = format!(
            "# comment\n{}  MIO-v0.3.0-macos-arm64.zip\n{} *dist/MIO-v0.3.0-linux-x86_64.tar.gz\n",
            "A".repeat(64),
            "b".repeat(64)
        );
        assert_eq!(
            parse_sha256_sums(&sums, "MIO-v0.3.0-macos-arm64.zip").as_deref(),
            Some("a".repeat(64).as_str())
        );
        assert_eq!(
            parse_sha256_sums(&sums, "MIO-v0.3.0-linux-x86_64.tar.gz").as_deref(),
            Some("b".repeat(64).as_str())
        );
        assert_eq!(parse_sha256_sums(&sums, "missing.zip"), None);
    }

    #[tokio::test]
    async fn updater_checks_follows_redirects_and_enforces_sha256() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");

        let suffix = current_platform_asset_suffixes()[0];
        let asset_name = format!("MIO-v9.9.9{suffix}");
        let good_payload = b"synthetic-mio-release-archive".to_vec();
        let good_hash = sha256_hex(&good_payload);
        let release_json = serde_json::json!({
            "tag_name": "v9.9.9",
            "html_url": "https://example.invalid/releases/v9.9.9",
            "body": "Bug fixes and polish.",
            "draft": false,
            "assets": [
                {
                    "name": asset_name,
                    "browser_download_url": format!("{base}/redirect-asset"),
                    "size": good_payload.len(),
                },
                {
                    "name": CHECKSUMS_ASSET_NAME,
                    "browser_download_url": format!("{base}/SHA256SUMS.txt"),
                    "size": 128,
                }
            ]
        })
        .to_string();
        let sums_body = format!("{good_hash}  {asset_name}\n");

        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let release_json = release_json.clone();
                let sums_body = sums_body.clone();
                let good_payload = good_payload.clone();
                let base = base.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, extra_hdr, body): (&str, String, Vec<u8>) = match path {
                        "/latest" => ("200 OK", String::new(), release_json.into_bytes()),
                        "/redirect-asset" => (
                            "302 Found",
                            format!("Location: {base}/real-asset\r\n"),
                            Vec::new(),
                        ),
                        "/real-asset" => ("200 OK", String::new(), good_payload),
                        "/SHA256SUMS.txt" => ("200 OK", String::new(), sums_body.into_bytes()),
                        "/tampered-asset" => ("200 OK", String::new(), b"corrupted".to_vec()),
                        _ => ("404 Not Found", String::new(), b"not found".to_vec()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra_hdr}Connection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                });
            }
        });

        let cancel = CancellationToken::new();
        let status = check_for_updates(&format!("http://{addr}/latest"), "0.2.0", &cancel).await;
        let UpdateStatus::Available(info) = status else {
            panic!("expected Available, got {status:?}");
        };
        assert_eq!(info.latest_version, "9.9.9");
        assert_eq!(info.asset_name, asset_name);
        assert_eq!(info.asset_sha256.as_deref(), Some(good_hash.as_str()));

        let downloaded = http_get_bytes(
            &info.asset_url,
            "*/*",
            Duration::from_secs(10),
            ASSET_MAX_BYTES,
            &cancel,
        )
        .await
        .unwrap();
        verify_archive_checksum(&info, &downloaded, &cancel)
            .await
            .expect("redirected download must pass SHA-256 verification");

        let mut tampered_info = info.clone();
        tampered_info.asset_url = format!("http://{addr}/tampered-asset");
        let tampered_bytes = http_get_bytes(
            &tampered_info.asset_url,
            "*/*",
            Duration::from_secs(10),
            ASSET_MAX_BYTES,
            &cancel,
        )
        .await
        .unwrap();
        let err = verify_archive_checksum(&tampered_info, &tampered_bytes, &cancel)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SHA-256 校验失败"));

        server.abort();
    }
}
