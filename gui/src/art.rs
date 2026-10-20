//! Game covers, from the Steam client's own cache on this PC. Steam keeps the
//! pictures of the games in its library in `appcache\librarycache`, so a game played
//! from Steam has one there. Nothing is downloaded: a game without one gets none.

use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

/// Pictures larger than this are not Steam's covers.
const MAX_BYTES: u64 = 8 << 20;

/// The pictures taken, best first: the wide one a store page shows, then the tall
/// capsule of the library, then its backdrop.
const NAMES: [&str; 5] = ["header.jpg", "library_header.jpg", "library_600x900.jpg", "library_capsule.jpg", "library_hero.jpg"];

/// The cover of the Steam app `id`: its content type and bytes.
pub fn steam(id: u32) -> Option<(&'static str, Vec<u8>)> {
    let file = find(&steam_dir()?.join("appcache").join("librarycache"), id)?;
    let body = std::fs::read(&file).ok()?;
    let png = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
    Some((if png { "image/png" } else { "image/jpeg" }, body))
}

/// Older Steam clients keep `<id>_header.jpg` in the cache folder itself; newer ones
/// a folder for each app, `<id>\header.jpg`, with some pictures a folder further down.
fn find(cache: &Path, id: u32) -> Option<PathBuf> {
    let dir = cache.join(id.to_string());
    let mut below: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    below.sort();
    let usable = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() <= MAX_BYTES);
    NAMES.iter().find_map(|name| {
        let mut places = vec![cache.join(format!("{id}_{name}")), dir.join(name)];
        places.extend(below.iter().map(|d| d.join(name)));
        places.into_iter().find(|p| usable(p))
    })
}

/// Where Steam is: what the Steam client wrote down, or its usual folder.
fn steam_dir() -> Option<PathBuf> {
    let found = string(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath")
        .or_else(|| string(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"))
        .map(PathBuf::from);
    let usual = || std::env::var_os("ProgramFiles(x86)").map(|p| PathBuf::from(p).join("Steam"));
    found.filter(|p| p.is_dir()).or_else(usual).filter(|p| p.is_dir())
}

/// A string value from the registry.
fn string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide(key), wide(value));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..(size as usize / 2)]);
    let s = s.trim_end_matches('\0');
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_found_in_either_layout_of_the_cache() {
        let cache = std::env::temp_dir().join(format!("camless-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let put = |p: &str| {
            let f = cache.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"\xff\xd8").unwrap();
        };
        // An older client: files named by the app.
        put("10_header.jpg");
        put("10_library_600x900.jpg");
        // A newer one: a folder for each app, a picture a folder further down.
        put(r"20\library_600x900.jpg");
        put(r"20\0a1b2c\header.jpg");
        put(r"30\library_hero.jpg");
        put(r"30\logo.png");
        std::fs::create_dir_all(cache.join("40")).unwrap();
        std::fs::write(cache.join(r"40\header.jpg"), b"").unwrap();
        let found = |id| find(&cache, id).map(|p| p.strip_prefix(&cache).unwrap().to_string_lossy().into_owned());
        assert_eq!(found(10).as_deref(), Some("10_header.jpg"));
        assert_eq!(found(20).as_deref(), Some(r"20\0a1b2c\header.jpg"));
        assert_eq!(found(30).as_deref(), Some(r"30\library_hero.jpg"));
        // An empty file is not a picture.
        assert_eq!(found(40), None);
        assert_eq!(found(50), None);
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
