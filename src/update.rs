//! Update check/download from GitHub Releases. WinHTTP + BCrypt from Windows; no extra crates.
//!
//! The release must carry an asset named `edgemin-lite.exe`; its SHA-256 comes from the GitHub API
//! `digest` field (`sha256:<hex>`). Without a digest the update is refused.
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows_sys::w;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET: &str = "edgemin-lite.exe";
const MAX_API: usize = 2 << 20;
const MAX_EXE: usize = 20 << 20;

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

// ---- pure logic ----

/// `owner/name` with GitHub-legal characters only (it is interpolated into a URL).
pub fn valid_repo(r: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && !s.bytes().all(|c| c == b'.')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let mut p = r.split('/');
    matches!((p.next(), p.next(), p.next()), (Some(a), Some(b), None) if ok(a) && ok(b))
}

/// Value of the first `"key":"..."` in `s`. GitHub's JSON has no escapes in the fields we read.
fn json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let rest = s[s.find(&pat)? + pat.len()..].trim_start().strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

pub fn parse_release(json: &str) -> Option<Release> {
    let version = json_str(json, "tag_name")?.to_string();
    let mut from = 0;
    while let Some(p) = json[from..].find("\"name\":") {
        let at = from + p;
        from = at + 7;
        if json_str(&json[at..], "name") != Some(ASSET) {
            continue;
        }
        // this asset's fields end where the next asset's "name" begins
        let end = json[from..].find("\"name\":").map_or(json.len(), |n| from + n);
        let asset = &json[at..end];
        let sha256 = json_str(asset, "digest")?.strip_prefix("sha256:")?.to_ascii_lowercase();
        let url = json_str(asset, "browser_download_url")?.to_string();
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) || !url.starts_with("https://") {
            return None;
        }
        return Some(Release { version, url, sha256 });
    }
    None
}

fn parse_ver(s: &str) -> Option<Vec<u64>> {
    s.trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok()).collect()
}

/// True when `remote` is strictly newer than `local`. Unparsable (e.g. pre-release) tags are never newer.
pub fn newer(remote: &str, local: &str) -> bool {
    let (Some(mut r), Some(mut l)) = (parse_ver(remote), parse_ver(local)) else { return false };
    let n = r.len().max(l.len());
    r.resize(n, 0);
    l.resize(n, 0);
    r > l
}

// ---- WinHTTP ----

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get(url: &str, max: usize) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("only https URLs are allowed")?;
    let (host, path) = rest.split_once('/').ok_or("bad URL")?;
    unsafe {
        let ua = w(&format!("edgemin-lite/{VERSION}"));
        let ses = Handle(WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if ses.0.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        WinHttpSetTimeouts(ses.0, 10_000, 10_000, 15_000, 30_000);
        let con = Handle(WinHttpConnect(ses.0, w(host).as_ptr(), 443, 0));
        if con.0.is_null() {
            return Err(format!("cannot connect to {host}"));
        }
        let req = Handle(WinHttpOpenRequest(
            con.0, w!("GET"), w(&format!("/{path}")).as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err("WinHttpOpenRequest failed".into());
        }
        let hdr = w("Accept: application/vnd.github+json\r\n");
        if WinHttpSendRequest(req.0, hdr.as_ptr(), u32::MAX, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(req.0, null_mut()) == 0
        {
            return Err("request failed (network error)".into());
        }
        let (mut code, mut len) = (0u32, 4u32);
        WinHttpQueryHeaders(
            req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, null(),
            (&mut code as *mut u32).cast(), &mut len, null_mut(),
        );
        if code != 200 {
            return Err(format!("server answered HTTP {code}"));
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut n = 0u32;
            if WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n) == 0 {
                return Err("download interrupted".into());
            }
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n as usize]);
            if out.len() > max {
                return Err("response too large".into());
            }
        }
    }
}

fn sha256_hex(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    let st = unsafe {
        BCryptHash(BCRYPT_SHA256_ALG_HANDLE, null_mut(), 0, data.as_ptr().cast_mut(), data.len() as u32, out.as_mut_ptr(), 32)
    };
    (st == 0).then(|| out.iter().map(|b| format!("{b:02x}")).collect())
}

// ---- public flow ----

/// `Ok(Some(release))` when the latest release is newer than this build.
pub fn check(repo: &str) -> Result<Option<Release>, String> {
    if !valid_repo(repo) {
        return Err("set [Update] Repo=owner/name in the config file".into());
    }
    let body = get(&format!("https://api.github.com/repos/{repo}/releases/latest"), MAX_API)?;
    let json = String::from_utf8_lossy(&body);
    let rel = parse_release(&json).ok_or("latest release has no edgemin-lite.exe asset with a sha256 digest")?;
    Ok(newer(&rel.version, VERSION).then_some(rel))
}

fn exe_path() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// Downloads next to the running exe and verifies the published SHA-256. Returns the staged file.
pub fn download(rel: &Release) -> Result<PathBuf, String> {
    let data = get(&rel.url, MAX_EXE)?;
    if !data.starts_with(b"MZ") {
        return Err("downloaded file is not a Windows executable".into());
    }
    if sha256_hex(&data).as_deref() != Some(rel.sha256.as_str()) {
        return Err("SHA-256 mismatch: download discarded".into());
    }
    let staged = exe_path()?.with_extension("exe.new");
    std::fs::write(&staged, &data).map_err(|e| format!("cannot write {}: {e}", staged.display()))?;
    Ok(staged)
}

/// Swaps the staged file in (a running exe can be renamed, not overwritten) and starts it with
/// `--wait` so it takes over the single-instance mutex once this process exits.
pub fn install(staged: &Path) -> Result<(), String> {
    let exe = exe_path()?;
    let old = exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("cannot replace the running exe: {e}"))?;
    if let Err(e) = std::fs::rename(staged, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("cannot move the new exe into place: {e}"));
    }
    if let Err(e) = std::process::Command::new(&exe).arg("--wait").spawn() {
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("cannot start the new version: {e}"));
    }
    Ok(())
}

/// Removes leftovers of a previous update.
pub fn cleanup() {
    if let Ok(exe) = exe_path() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
        let _ = std::fs::remove_file(exe.with_extension("exe.new"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn json(second_digest: &str) -> String {
        format!(
            r#"{{"tag_name":"v0.2.0","assets":[{{"url":"https://api.github.com/a/1","name":"notes.txt","digest":"sha256:{}","browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/notes.txt"}},{{"url":"https://api.github.com/a/2","name":"edgemin-lite.exe","size":1,{second_digest}"browser_download_url":"https://github.com/o/r/releases/download/v0.2.0/edgemin-lite.exe"}}]}}"#,
            "f".repeat(64)
        )
    }

    #[test]
    fn picks_the_exe_asset_and_its_own_digest() {
        let r = parse_release(&json(&format!(r#""digest":"sha256:{HASH}","#))).unwrap();
        assert_eq!((r.version.as_str(), r.sha256.as_str()), ("v0.2.0", HASH));
        assert!(r.url.ends_with("/edgemin-lite.exe"));
    }

    #[test]
    fn refuses_exe_asset_without_digest() {
        // the neighbouring asset's digest must not be borrowed
        assert!(parse_release(&json("")).is_none());
        assert!(parse_release(&json(r#""digest":"sha256:zz","#)).is_none());
    }

    #[test]
    fn version_ordering() {
        assert!(newer("v0.2.0", "0.1.0") && newer("0.10.0", "0.9.9") && newer("1.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0") && !newer("0.1", "0.1.0") && !newer("0.0.9", "0.1.0"));
        assert!(!newer("v0.2.0-beta", "0.1.0"));
    }

    #[test]
    fn repo_names() {
        assert!(valid_repo("diego-ruas/edgemin-lite"));
        for bad in ["", "a", "a/b/c", "a/", "/b", "a/b?x=1", "a b/c", "../x"] {
            assert!(!valid_repo(bad), "{bad}");
        }
    }
}

#[cfg(test)]
mod sha_tests {
    #[test]
    fn sha256_known_vectors() {
        assert_eq!(super::sha256_hex(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(super::sha256_hex(b"").unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
