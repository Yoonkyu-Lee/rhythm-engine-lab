//! 라이브러리 색인 `index.bin` (명세 5-3): 선곡 화면이 곡 묶음을 열지 않고 읽는 파일 하나.
//!
//! ```text
//! "TIDX" | major u16 | minor u16 | count u32
//! Track마다: Track ID [16] | revision u32 | 묶음 해시 [32] | 묶음 크기 u64 | 상대 경로 (u32 길이 + UTF-8)
//!            INFO 구역 바이트 (u32 길이 + 바이트) | STATS 구역 바이트 (u32 길이 + 바이트)
//! ```
//! Track 순서는 상대 경로 순. 같은 묶음 집합이면 같은 바이트.

use crate::compile::{FORMAT_MAJOR, FORMAT_MINOR};
use crate::pack::{Bundle, W, K_INFO, K_STATS};
use std::path::Path;

pub fn build_index(lib: &Path) -> Result<usize, String> {
    let dir = lib.join("tracks");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "trk").unwrap_or(false))
        .collect();
    paths.sort();
    let mut w = W::default();
    w.bytes(b"TIDX");
    w.u16(FORMAT_MAJOR);
    w.u16(FORMAT_MINOR);
    w.u32(paths.len() as u32);
    for p in &paths {
        let b = Bundle::open(std::fs::read(p).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", p.display()))?;
        w.bytes(&b.track_id);
        w.u32(b.revision);
        w.bytes(&b.bytes[72..104]);
        w.u64(b.bytes.len() as u64);
        w.str(&format!("tracks/{}", p.file_name().unwrap().to_string_lossy()));
        for k in [K_INFO, K_STATS] {
            let s = b.section(k, 0).ok_or("INFO/STATS 없음")?;
            w.u32(s.len() as u32);
            w.bytes(s);
        }
    }
    std::fs::write(lib.join("index.bin"), &w.0).map_err(|e| e.to_string())?;
    Ok(paths.len())
}
