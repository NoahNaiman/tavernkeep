//! Optional on-disk snapshot of cached spells and monsters (`TK_CACHE_FILE`),
//! so a restart keeps the table working while Open5e is unreachable.
//!
//! Each entry stores its age at save time. On load, the wall-clock time
//! since the save is added, so fresh/stale timing carries across restarts.
//! Negative (404) entries and search results are not saved.

use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::open5e::{Monster, Spell};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved<T> {
    pub slug: String,
    pub age_ms: u64,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub saved_at_unix_ms: u64,
    pub spells: Vec<Saved<Spell>>,
    pub monsters: Vec<Saved<Monster>>,
}

pub fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl Snapshot {
    /// Wall-clock time since this snapshot was written (zero if the clock
    /// went backwards).
    pub fn elapsed_since_save(&self) -> Duration {
        Duration::from_millis(unix_ms_now().saturating_sub(self.saved_at_unix_ms))
    }
}

/// Write atomically: a temp file in the same directory, then rename.
pub fn save(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec(snapshot).map_err(io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// `Ok(None)` when there is no snapshot yet.
pub fn load(path: &Path) -> io::Result<Option<Snapshot>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let snapshot: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if snapshot.version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported snapshot version {}", snapshot.version),
        ));
    }
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tk-snapshot-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("cache.json")
    }

    fn sample() -> Snapshot {
        Snapshot {
            version: VERSION,
            saved_at_unix_ms: unix_ms_now(),
            spells: vec![Saved {
                slug: "x".into(),
                age_ms: 5,
                value: Spell {
                    slug: "x".into(),
                    name: "X".into(),
                    level: Some(1),
                    school: None,
                    casting_time: None,
                    range: None,
                    duration: None,
                    components: None,
                    concentration: None,
                    ritual: None,
                    description: None,
                },
            }],
            monsters: vec![],
        }
    }

    #[test]
    fn round_trips_through_a_file() {
        let path = temp_path("roundtrip");
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path).unwrap(), Some(sample()));
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn missing_file_is_none() {
        assert_eq!(
            load(&temp_path("missing").with_file_name("nope.json")).unwrap(),
            None
        );
    }

    #[test]
    fn corrupt_or_wrong_version_is_an_error() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let mut s = sample();
        s.version = 99;
        std::fs::write(&path, serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(load(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn elapsed_since_save_uses_wall_time() {
        let mut s = sample();
        s.saved_at_unix_ms -= 90_000;
        assert!(s.elapsed_since_save() >= Duration::from_secs(90));
        s.saved_at_unix_ms = u64::MAX;
        assert_eq!(s.elapsed_since_save(), Duration::ZERO);
    }
}
