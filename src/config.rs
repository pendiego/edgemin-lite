use std::fs;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub browser_path: String,
    pub enabled: bool,
    pub interval_ms: u32,
    pub limit_enabled: bool,
    pub limit_mb: u64,
    pub start_browser: bool,
    pub extended_enabled: bool,
    pub extended: Vec<String>,
    pub safemode: Vec<(String, String)>,
    /// GitHub `owner/name` whose latest release is checked for updates; empty disables the updater.
    pub update_repo: String,
    pub update_check: bool,
}

/// Where updates come from unless the INI sets `[Update] Repo=` (empty disables updates).
pub const DEFAULT_REPO: &str = "pendiego/edgemin-lite";

pub const DEFAULT_INI: &str = "[Edgemin]\r\n\
BrowserPath=C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\r\n\
BoostEnabled=1\r\n\
Boost=500\r\n\
LimitEnabled=1\r\n\
ReduceLimit=10\r\n\
StartBrowser=0\r\n\
EnableExtendedProcs=1\r\n\
ExtendedProcs=\r\n\
\r\n\
[Safemode]\r\n\
firefox.exe=-safe-mode\r\n\
waterfox.exe=-safe-mode\r\n\
chrome.exe=--disable-extensions\r\n\
msedge.exe=-inprivate\r\n\
\r\n\
[Update]\r\n\
Repo=pendiego/edgemin-lite\r\n\
Check=1\r\n";

fn flag(v: &str, default: bool) -> bool {
    match v {
        "1" => true,
        "0" => false,
        _ => default,
    }
}

pub fn parse(bytes: &[u8]) -> Config {
    let text = if bytes.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        String::from_utf8_lossy(b).into_owned()
    };

    let mut c = Config {
        browser_path: r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe".into(),
        enabled: true,
        interval_ms: 500,
        limit_enabled: true,
        limit_mb: 10,
        start_browser: false,
        extended_enabled: true,
        extended: Vec::new(),
        safemode: Vec::new(),
        update_repo: DEFAULT_REPO.into(),
        update_check: true,
    };
    let mut section = String::new();
    for line in text.split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.trim().to_ascii_lowercase();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match section.as_str() {
            "safemode" => c.safemode.push((k.to_string(), v.to_string())),
            "edgemin" => match k.to_ascii_lowercase().as_str() {
                "browserpath" if !v.is_empty() => c.browser_path = v.to_string(),
                "boostenabled" => c.enabled = flag(v, c.enabled),
                "boost" => {
                    if let Ok(n) = v.parse::<u32>() {
                        c.interval_ms = n.max(100)
                    }
                }
                "limitenabled" => c.limit_enabled = flag(v, c.limit_enabled),
                "reducelimit" => {
                    if let Ok(n) = v.parse() {
                        c.limit_mb = n
                    }
                }
                "startbrowser" => c.start_browser = flag(v, c.start_browser),
                "enableextendedprocs" => c.extended_enabled = flag(v, c.extended_enabled),
                "extendedprocs" => {
                    c.extended = v
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                }
                _ => {}
            },
            "update" => match k.to_ascii_lowercase().as_str() {
                "repo" => c.update_repo = v.to_string(),
                "check" => c.update_check = flag(v, c.update_check),
                _ => {}
            },
            _ => {}
        }
    }
    c
}

impl Config {
    pub fn core_exe(&self) -> &str {
        self.browser_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&self.browser_path)
    }

    pub fn safemode_args(&self) -> Option<&str> {
        let exe = self.core_exe();
        self.safemode
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(exe))
            .map(|(_, v)| v.as_str())
    }
}

pub fn path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.with_file_name("edgemin-lite.ini");
        if p.exists() {
            return p;
        }
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join("edgemin-lite").join("edgemin-lite.ini")
}

pub fn load() -> Config {
    let p = path();
    if !p.exists() {
        if let Some(d) = p.parent() {
            let _ = fs::create_dir_all(d);
        }
        let _ = fs::write(&p, DEFAULT_INI);
    }
    match fs::read(&p) {
        Ok(b) => parse(&b),
        Err(_) => parse(DEFAULT_INI.as_bytes()),
    }
}

impl Config {
    /// Known keys plus [Safemode]; unknown keys/sections of an old INI are not preserved.
    pub fn to_ini(&self) -> String {
        let b = |v: bool| if v { 1 } else { 0 };
        let mut s = format!(
            "[Edgemin]\r\nBrowserPath={}\r\nBoostEnabled={}\r\nBoost={}\r\nLimitEnabled={}\r\nReduceLimit={}\r\nStartBrowser={}\r\nEnableExtendedProcs={}\r\nExtendedProcs={}\r\n\r\n[Safemode]\r\n",
            self.browser_path, b(self.enabled), self.interval_ms, b(self.limit_enabled), self.limit_mb,
            b(self.start_browser), b(self.extended_enabled), self.extended.join(",")
        );
        for (k, v) in &self.safemode {
            s.push_str(&format!("{k}={v}\r\n"));
        }
        s.push_str(&format!("\r\n[Update]\r\nRepo={}\r\nCheck={}\r\n", self.update_repo, b(self.update_check)));
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        fs::write(p, self.to_ini())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_bom() {
        let s = "[Edgemin]\r\nBoost=50\r\nExtendedProcs= a.exe , ,b.exe\r\n";
        let mut b = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        let c = parse(&b);
        assert_eq!(c.interval_ms, 100);
        assert_eq!(c.extended, ["a.exe", "b.exe"]);
    }

    #[test]
    fn real_ini() {
        let s = "[Edgemin]\r\nBrowserPath=C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\r\nReduceLimit=100\r\nCheckForUpdates=1\r\n[Safemode]\r\nfirefox.exe=-safe-mode\r\nmsedge.exe=-inprivate\r\n";
        let c = parse(s.as_bytes());
        assert_eq!(c.limit_mb, 100);
        assert_eq!(c.safemode_args(), Some("-inprivate"));
    }

    #[test]
    fn defaults() {
        let c = parse(b"");
        assert!(c.enabled && c.limit_enabled && c.extended_enabled && !c.start_browser);
        assert_eq!((c.interval_ms, c.limit_mb), (500, 10));
        assert!(c.extended.is_empty());
    }

    #[test]
    fn to_ini_roundtrip() {
        let c = parse(b"[Edgemin]\r\nBrowserPath=C:\\x\\a.exe\r\nBoostEnabled=0\r\nBoost=900\r\nReduceLimit=7\r\nStartBrowser=1\r\nExtendedProcs=p.exe,q.exe\r\n[Safemode]\r\na.exe=-s\r\n");
        let d = parse(c.to_ini().as_bytes());
        assert_eq!((d.browser_path.as_str(), d.enabled, d.interval_ms, d.limit_mb, d.start_browser), ("C:\\x\\a.exe", false, 900, 7, true));
        assert_eq!(d.extended, ["p.exe", "q.exe"]);
        assert_eq!(d.safemode_args(), Some("-s"));
    }
}

#[cfg(test)]
mod update_cfg_tests {
    use super::*;

    #[test]
    fn default_repo_is_used_and_empty_disables() {
        assert_eq!(parse(b"").update_repo, DEFAULT_REPO);
        assert_eq!(parse(b"[Update]\r\nRepo=\r\n").update_repo, "");
        assert_eq!(parse(DEFAULT_INI.as_bytes()).update_repo, DEFAULT_REPO);
    }
}
