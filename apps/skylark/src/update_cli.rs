//! Retained application updater, blocked before any I/O in local-only builds.

use anyhow::bail;
use skylark_update::{InstallKind, current_version, version_newer};

/// `--check` prints the verdict and exits (nonzero when an update is available,
/// so scripts can gate on it).
pub async fn update(edge_url: &str, check_only: bool) -> anyhow::Result<()> {
    anyhow::ensure!(!skylark_engine::LOCAL_ONLY_BUILD, skylark_engine::LOCAL_ONLY_MESSAGE);
    let manifest = skylark_update::fetch_latest(edge_url).await?;
    let current = current_version();
    if !version_newer(&manifest.version, current) {
        println!(
            "skylark {current} is up to date (latest: {}).",
            manifest.version
        );
        return Ok(());
    }
    println!("skylark {current} → {} available", manifest.version);
    if check_only {
        std::process::exit(1);
    }

    match skylark_update::detect_install() {
        InstallKind::Managed { app_root } => {
            println!(
                "downloading {}…",
                skylark_update::headless_artifact(&manifest.version)
            );
            skylark_update::stage_headless(edge_url, &manifest, &app_root).await?;
            skylark_update::apply_headless(&app_root, &manifest.version)?;
            println!(
                "installed {} (current → {})",
                app_root.join(&manifest.version).display(),
                manifest.version
            );
            match skylark_update::restart_service() {
                Ok(()) => println!("engine service restarted."),
                Err(err) => println!(
                    "note: service restart failed ({err:#}) — restart the engine manually to finish."
                ),
            }
            Ok(())
        }
        InstallKind::MacApp { bundle } => {
            println!(
                "downloading {}…",
                skylark_update::mac_app_artifact(&manifest.version)
            );
            let data_dir = super::paths::data_dir();
            let staged = skylark_update::stage_mac_app(edge_url, &manifest, &data_dir).await?;
            skylark_update::apply_mac_app(&staged, &bundle)?;
            println!("updated {} — relaunch Skylark to finish.", bundle.display());
            Ok(())
        }
        #[cfg(windows)]
        InstallKind::WindowsPortable { directory } => {
            let staged = skylark_update::windows::stage(edge_url, &manifest, &directory).await?;
            skylark_update::windows::apply(&staged, &directory, false)?;
            println!(
                "updated to {} — relaunch Skylark to finish.",
                manifest.version
            );
            Ok(())
        }
        InstallKind::Unmanaged => {
            bail!("this binary is not update-managed; rebuild Skylark from this checkout.")
        }
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn update_and_check_are_disabled_before_network_access() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        for check_only in [false, true] {
            let error = super::update(&url, check_only).await.unwrap_err();
            assert!(error.to_string().contains(skylark_engine::LOCAL_ONLY_MESSAGE));
        }
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(50), listener.accept()
        ).await.is_err());
    }
}
