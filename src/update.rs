//! `flowcli --self-update`: re-run the published installer into the folder
//! the running binary lives in.
//!
//! Downloading, checksum verification and the sudo/quarantine handling all
//! live in `install.sh`; this only hands it the right install dir. Shelling
//! out to `curl | bash` keeps flowcli free of an HTTP/TLS stack — the trust
//! model is the same HTTPS-to-GitHub as the documented one-line install.

use std::process::Command;

/// Where the installer is published. Overridable via `FLOWCLI_INSTALLER_URL`
/// (e.g. to test a fork or a branch's install.sh before it reaches `main`).
const INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/go-routine-id/flowcli/main/install.sh";
const INSTALLER_URL_ENV: &str = "FLOWCLI_INSTALLER_URL";
/// Read by install.sh; set here so the upgrade replaces *this* binary.
const INSTALL_DIR_ENV: &str = "FLOWCLI_INSTALL_DIR";
const RELEASES_PAGE: &str = "https://github.com/go-routine-id/flowcli/releases/latest";

/// Returns the process exit code: 0 when the installer succeeded.
pub fn self_update() -> i32 {
    if cfg!(windows) {
        eprintln!(
            "--self-update is for macOS and Linux. On Windows, download \
             flowcli-windows-x86_64.exe from\n  {RELEASES_PAGE}\nand replace \
             your flowcli.exe with it."
        );
        return 1;
    }

    let dir = match std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| p.parent().map(|d| d.to_path_buf()))
    {
        Ok(Some(d)) => d,
        Ok(None) | Err(_) => {
            eprintln!("cannot locate the running flowcli binary");
            return 1;
        }
    };
    let url = std::env::var(INSTALLER_URL_ENV).unwrap_or_else(|_| INSTALLER_URL.to_string());

    // The URL goes in as "$1", never spliced into the script text.
    let status = Command::new("bash")
        .args([
            "-o",
            "pipefail",
            "-c",
            r#"curl -fsSL "$1" | bash"#,
            "flowcli-update",
        ])
        .arg(&url)
        .env(INSTALL_DIR_ENV, &dir)
        .status();
    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("cannot run bash for the installer: {e}");
            1
        }
    }
}
