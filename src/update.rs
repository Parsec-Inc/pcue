//! Atualização pelas releases do GitHub; procura sozinho, instala só a pedido.

use std::process::{Command, Stdio};
use std::sync::mpsc;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::constants::{UPDATE_CHECKSUMS, UPDATE_REPO};
use crate::platform;
use crate::utils::in_background;

/// Release publicada, com os arquivos para baixar.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub version: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    /// É mais nova que o binário em execução?
    pub fn is_newer(&self) -> bool {
        parse_version(&self.version) > parse_version(env!("CARGO_PKG_VERSION"))
    }

    fn asset(&self, name: &str) -> Result<&Asset> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .with_context(|| format!("a release {} não tem {name}", self.version))
    }
}

/// Versão `x.y.z` comparável; partes inválidas contam como zero.
fn parse_version(raw: &str) -> Vec<u64> {
    raw.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Resultado das tarefas de atualização feitas em segundo plano.
pub enum UpdateEvent {
    Available(Release),
    Installed(String),
    Failed(String),
}

/// Procura versão nova numa thread; sem rede, fica quieto.
pub fn check_in_background() -> mpsc::Receiver<UpdateEvent> {
    in_background(|| latest().ok().filter(Release::is_newer).map(UpdateEvent::Available))
}

/// Instala a release numa thread e avisa quando terminar.
pub fn install_in_background(release: Release) -> mpsc::Receiver<UpdateEvent> {
    in_background(move || {
        Some(match install(&release) {
            Ok(()) => UpdateEvent::Installed(release.version),
            Err(err) => UpdateEvent::Failed(format!("{err:#}")),
        })
    })
}

/// `pcue update`: mostra ou instala a versão nova.
pub fn run(check: bool) -> Result<()> {
    let release = latest()?;
    let current = env!("CARGO_PKG_VERSION");
    if !release.is_newer() {
        println!("pcue {current} já é a versão mais nova");
    } else if check {
        println!("versão {} disponível (atual {current}); rode `pcue update`", release.version);
    } else {
        println!("baixando pcue {}...", release.version);
        install(&release)?;
        println!("atualizado de {current} para {}; o daemon se reinicia sozinho", release.version);
    }
    Ok(())
}

/// Última release publicada.
fn latest() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest");
    let body = download(&url).context("não foi possível consultar as releases")?;
    serde_json::from_slice(&body).context("resposta inesperada do GitHub")
}

/// Baixa o binário da plataforma, confere o SHA-256 e troca o executável atual.
fn install(release: &Release) -> Result<()> {
    let exe = std::env::current_exe().context("não foi possível achar o próprio executável")?;
    install_to(release, &exe)
}

fn install_to(release: &Release, exe: &std::path::Path) -> Result<()> {
    let name = format!("pcue-{}-{}", release.version, platform::ASSET_SUFFIX);
    let sums = String::from_utf8(download(&release.asset(UPDATE_CHECKSUMS)?.browser_download_url)?)?;
    let expected = sums
        .lines()
        .find_map(|line| line.split_once("  ").filter(|(_, file)| *file == name).map(|(sum, _)| sum.to_string()))
        .with_context(|| format!("{name} não está no {UPDATE_CHECKSUMS}"))?;
    let bytes = download(&release.asset(&name)?.browser_download_url)?;
    if sha256_hex(&bytes) != expected {
        bail!("o arquivo baixado não confere com o {UPDATE_CHECKSUMS}");
    }
    platform::replace_exe(exe, &bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Baixa a URL com o `curl` do sistema (Linux e Windows 10+ já trazem).
fn download(url: &str) -> Result<Vec<u8>> {
    let mut command = Command::new("curl");
    command
        .args(["-fsSL", "--max-time", "120", "-H", "Accept: application/vnd.github+json", url])
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    platform::background(&mut command);
    let output = command.output().context("curl não encontrado")?;
    if !output.status.success() {
        bail!("falha ao baixar {url}: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Release falsa servida por `file://`.
    fn fake_release(dir: &std::path::Path, sums_ok: bool) -> Release {
        let name = format!("pcue-9.9.9-{}", platform::ASSET_SUFFIX);
        std::fs::write(dir.join(&name), b"novo").unwrap();
        let sum = sha256_hex(if sums_ok { b"novo" } else { b"outro" });
        std::fs::write(dir.join(UPDATE_CHECKSUMS), format!("{sum}  {name}\n")).unwrap();
        let url = |file: &str| {
            let path = dir.join(file).display().to_string().replace('\\', "/");
            format!("file://{}{path}", if path.starts_with('/') { "" } else { "/" })
        };
        Release {
            version: "9.9.9".into(),
            assets: [name.as_str(), UPDATE_CHECKSUMS]
                .map(|n| Asset { name: n.into(), browser_download_url: url(n) })
                .into(),
        }
    }

    #[test]
    fn install_checks_sum_and_replaces() {
        let dir = std::env::temp_dir().join(format!("pcue-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("pcue-bin");
        std::fs::write(&exe, b"velho").unwrap();
        assert!(install_to(&fake_release(&dir, false), &exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"velho");
        install_to(&fake_release(&dir, true), &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"novo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("0.10.0") > parse_version("0.9.3"));
        assert!(parse_version("v1.0.0") > parse_version("0.99.99"));
        assert_eq!(parse_version("0.2.0"), parse_version("0.2.0"));
    }
}
