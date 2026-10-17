//! 설정 저장 (3-4, `cp3/설정창_계획.md` 2장): `settings.toml` 하나, 범위별 표.
//!
//! ```toml
//! [global]                         # 전역
//! audio = "shared"                 # shared | exclusive | asio
//! render = "Playback 1/2"
//! volume_master = 1.0
//! [device."shared:Playback 1/2"]   # 장치별: D·오프셋
//! d_auto = true
//! d_margin_ms = 2.0
//! offset_a_ms = 0.0                # 판정 + 키음
//! offset_b_ms = 0.0                # 화면
//! [keymode."SC K1 K2 K3 K4 K5 K6 K7"]   # 키 모드별 (레인 id 서명)
//! keys = [[160], [90], [83], …]   # 레인마다 가상 키 여러 개
//! visible_ms = 600.0
//! [preset.default]                 # 규칙 (CP3는 하나)
//! gauge = "normal"
//! ```
//! - 읽기: 없는 키는 기본값, 범위 밖·타입이 틀린 값은 기본값 + 경고. 모르는 키는 그대로 남겨 다시 쓸 때 지우지 않는다.
//!   파일이 없으면(NotFound) 조용히 빈 문서, 그 밖의 읽기 실패·UTF-8 아님·TOML 오류는 원본을 고유한 `.broken-<시각>`으로 옮기고 경고(리뷰 3-4 #6).
//! - 쓰기: 같은 폴더에 고유한 임시 파일을 새로 만들어(create_new) 쓰고 바꾼다(도중에 꺼져도 깨지지 않음, 동시 저장끼리 충돌 없음 — 리뷰 3-4 #1).

use std::path::{Path, PathBuf};

use toml::{Table, Value};

pub struct Settings {
    pub path: PathBuf,
    doc: Table,
    /// 읽으며 기본값으로 되돌린 것들 (화면에 알림)
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub d_auto: bool,
    pub d_margin_ms: f64,
    pub d_manual_ms: f64,
    pub offset_a_ms: f64,
    pub offset_b_ms: f64,
}

impl Default for Device {
    fn default() -> Device {
        Device { d_auto: true, d_margin_ms: 2.0, d_manual_ms: 20.0, offset_a_ms: 0.0, offset_b_ms: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyMode {
    /// 레인마다 가상 키들 (비면 기본 배치)
    pub keys: Vec<Vec<u16>>,
    pub visible_ms: f64,
    /// 기준 스크롤: chart(차분 필드) | start | mode | max | min (`Chart::scroll_base`)
    pub scroll_base: String,
}

pub const SCROLL_BASES: [&str; 5] = ["chart", "start", "mode", "max", "min"];
pub const EMPTY_SOUNDS: [&str; 4] = ["chart", "previous", "next", "none"];

#[derive(Clone, Debug, PartialEq)]
pub struct Global {
    pub audio: String,
    pub render: String,
    pub excl_period_ms: f64,
    pub asio_buffer: i64,
    pub volume_master: f64,
    pub volume_bgm: f64,
    pub volume_keys: f64,
    pub late_drop_ms: f64,
    /// 빈 누름 소리: chart(차분 규칙) | previous | next | none
    pub empty_sound: String,
    pub library: String,
}

impl Default for Global {
    fn default() -> Global {
        Global {
            audio: "shared".into(),
            render: "Playback 1/2".into(),
            excl_period_ms: 5.0,
            asio_buffer: 64,
            volume_master: 1.0,
            volume_bgm: 1.0,
            volume_keys: 1.0,
            late_drop_ms: 100.0,
            empty_sound: "chart".into(),
            library: "content/library".into(),
        }
    }
}

fn num(t: &Table, k: &str, lo: f64, hi: f64, def: f64, w: &mut Vec<String>, at: &str) -> f64 {
    match t.get(k) {
        None => def,
        Some(v) => match v.as_float().or_else(|| v.as_integer().map(|i| i as f64)) {
            Some(x) if x.is_finite() && (lo..=hi).contains(&x) => x,
            _ => {
                w.push(format!("{at}.{k}: 범위 밖 또는 수가 아님 ({v}) → 기본값 {def}"));
                def
            }
        },
    }
}

fn text(t: &Table, k: &str, ok: &[&str], def: &str, w: &mut Vec<String>, at: &str) -> String {
    match t.get(k) {
        None => def.to_string(),
        Some(Value::String(s)) if ok.is_empty() || ok.contains(&s.as_str()) => s.clone(),
        Some(v) => {
            w.push(format!("{at}.{k}: 모르는 값 또는 문자열이 아님 ({v}) → 기본값 \"{def}\""));
            def.to_string()
        }
    }
}

fn boolean(t: &Table, k: &str, def: bool, w: &mut Vec<String>, at: &str) -> bool {
    match t.get(k) {
        None => def,
        Some(Value::Boolean(b)) => *b,
        Some(v) => {
            w.push(format!("{at}.{k}: 참/거짓이 아님 ({v}) → 기본값 {def}"));
            def
        }
    }
}

/// 같은 폴더의 고유한 새 파일 경로 (`<이름>.<꼬리>-<pid>-<ns>[-n]`). 이미 있는 파일은 절대 고르지 않는다.
pub fn unique_sibling(path: &Path, tail: &str) -> PathBuf {
    let ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let base = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "settings.toml".into());
    (0u32..)
        .map(|i| path.with_file_name(format!("{base}.{tail}-{}-{ns}{}", std::process::id(), if i == 0 { String::new() } else { format!("-{i}") })))
        .find(|p| !p.exists())
        .unwrap()
}

impl Settings {
    /// 읽기. 파일이 없으면 빈 문서, 읽을 수 없거나 깨졌으면 원본을 고유한 `.broken-…`으로 옮기고 빈 문서(경고, 옮긴 결과 그대로).
    pub fn load(path: &Path) -> Settings {
        let mut warnings = Vec::new();
        let name = path.display();
        let bad = |why: String, warnings: &mut Vec<String>| {
            let to = unique_sibling(path, "broken");
            let moved = match std::fs::rename(path, &to) {
                Ok(()) => format!("원본은 {}", to.display()),
                Err(e) => format!("원본을 옮기지 못함({e}) — 저장하면 덮어쓰니 먼저 따로 보관하세요"),
            };
            warnings.push(format!("{name}을 읽을 수 없어 기본값으로 시작 ({why}); {moved}"));
            Table::new()
        };
        let doc = match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Table::new(),
            Err(e) => {
                warnings.push(format!("{name}을 열 수 없어 기본값으로 시작 ({e}); 원본은 그대로 둠"));
                Table::new()
            }
            Ok(b) => match String::from_utf8(b) {
                Err(_) => bad("UTF-8이 아님".into(), &mut warnings),
                Ok(s) => match s.parse::<Table>() {
                    Ok(t) => t,
                    Err(e) => bad(e.to_string().lines().next().unwrap_or("").to_string(), &mut warnings),
                },
            },
        };
        Settings { path: path.to_path_buf(), doc, warnings }
    }

    fn warn(&mut self, m: String) {
        if !self.warnings.contains(&m) {
            self.warnings.push(m);
        }
    }

    /// 범위 표 읽기 (없으면 빈 표, 표가 아니면 경고 + 빈 표)
    fn table(&mut self, a: &str, b: Option<&str>) -> Table {
        let t = match self.doc.get(a) {
            None => Table::new(),
            Some(Value::Table(t)) => t.clone(),
            Some(v) => {
                self.warn(format!("[{a}]가 표가 아님 ({}) → 기본값", v.type_str()));
                Table::new()
            }
        };
        match b {
            None => t,
            Some(k) => match t.get(k) {
                None => Table::new(),
                Some(Value::Table(u)) => u.clone(),
                Some(v) => {
                    self.warn(format!("[{a}.\"{k}\"]가 표가 아님 ({}) → 기본값", v.type_str()));
                    Table::new()
                }
            },
        }
    }

    fn table_mut(&mut self, a: &str, b: Option<&str>) -> &mut Table {
        let t = self.doc.entry(a.to_string()).or_insert_with(|| Value::Table(Table::new()));
        if !t.is_table() {
            *t = Value::Table(Table::new());
        }
        let t = t.as_table_mut().unwrap();
        match b {
            None => t,
            Some(k) => {
                let u = t.entry(k.to_string()).or_insert_with(|| Value::Table(Table::new()));
                if !u.is_table() {
                    *u = Value::Table(Table::new());
                }
                u.as_table_mut().unwrap()
            }
        }
    }

    pub fn global(&mut self) -> Global {
        let t = self.table("global", None);
        let d = Global::default();
        let w = &mut self.warnings;
        Global {
            audio: text(&t, "audio", &["shared", "exclusive", "asio"], &d.audio, w, "global"),
            render: text(&t, "render", &[], &d.render, w, "global"),
            excl_period_ms: num(&t, "excl_period_ms", 2.0, 50.0, d.excl_period_ms, w, "global"),
            asio_buffer: num(&t, "asio_buffer", 16.0, 4096.0, d.asio_buffer as f64, w, "global") as i64,
            volume_master: num(&t, "volume_master", 0.0, 2.0, d.volume_master, w, "global"),
            volume_bgm: num(&t, "volume_bgm", 0.0, 2.0, d.volume_bgm, w, "global"),
            volume_keys: num(&t, "volume_keys", 0.0, 2.0, d.volume_keys, w, "global"),
            late_drop_ms: num(&t, "late_drop_ms", 0.0, 1000.0, d.late_drop_ms, w, "global"),
            empty_sound: text(&t, "empty_sound", &EMPTY_SOUNDS, &d.empty_sound, w, "global"),
            library: text(&t, "library", &[], &d.library, w, "global"),
        }
    }

    pub fn set_global(&mut self, g: &Global) {
        let t = self.table_mut("global", None);
        t.insert("audio".into(), Value::String(g.audio.clone()));
        t.insert("render".into(), Value::String(g.render.clone()));
        t.insert("excl_period_ms".into(), Value::Float(g.excl_period_ms));
        t.insert("asio_buffer".into(), Value::Integer(g.asio_buffer));
        t.insert("volume_master".into(), Value::Float(g.volume_master));
        t.insert("volume_bgm".into(), Value::Float(g.volume_bgm));
        t.insert("volume_keys".into(), Value::Float(g.volume_keys));
        t.insert("late_drop_ms".into(), Value::Float(g.late_drop_ms));
        t.insert("empty_sound".into(), Value::String(g.empty_sound.clone()));
        t.insert("library".into(), Value::String(g.library.clone()));
    }

    /// 장치 열쇠 = "<경로>:<장치 이름>"
    pub fn device(&mut self, key: &str) -> Device {
        let t = self.table("device", Some(key));
        let d = Device::default();
        let w = &mut self.warnings;
        let at = format!("device.{key}");
        Device {
            d_auto: boolean(&t, "d_auto", d.d_auto, w, &at),
            d_margin_ms: num(&t, "d_margin_ms", 0.0, 50.0, d.d_margin_ms, w, &at),
            d_manual_ms: num(&t, "d_manual_ms", 0.0, 200.0, d.d_manual_ms, w, &at),
            offset_a_ms: num(&t, "offset_a_ms", -100.0, 100.0, d.offset_a_ms, w, &at),
            offset_b_ms: num(&t, "offset_b_ms", -200.0, 200.0, d.offset_b_ms, w, &at),
        }
    }

    pub fn set_device(&mut self, key: &str, d: &Device) {
        let t = self.table_mut("device", Some(key));
        t.insert("d_auto".into(), Value::Boolean(d.d_auto));
        t.insert("d_margin_ms".into(), Value::Float(d.d_margin_ms));
        t.insert("d_manual_ms".into(), Value::Float(d.d_manual_ms));
        t.insert("offset_a_ms".into(), Value::Float(d.offset_a_ms));
        t.insert("offset_b_ms".into(), Value::Float(d.offset_b_ms));
    }

    /// 키 모드 열쇠 = 레인 id를 공백으로 이은 것
    pub fn keymode(&mut self, key: &str, lanes: usize) -> KeyMode {
        let t = self.table("keymode", Some(key));
        let at = format!("keymode.{key}");
        let mut keys: Vec<Vec<u16>> = Vec::new();
        match t.get("keys") {
            None => {}
            Some(Value::Array(arr)) => {
                for (i, l) in arr.iter().enumerate() {
                    let Some(a) = l.as_array() else {
                        self.warn(format!("{at}.keys[{i}]: 배열이 아님 ({l}) → 기본 배치"));
                        keys.clear();
                        break;
                    };
                    let mut ks = Vec::new();
                    for x in a {
                        match x.as_integer() {
                            Some(v) if (1..=0xFE).contains(&v) => ks.push(v as u16),
                            _ => self.warn(format!("{at}.keys[{i}]: 가상 키가 아님 ({x}) → 뺌")),
                        }
                    }
                    keys.push(ks);
                }
                if !keys.is_empty() && keys.len() != lanes {
                    self.warn(format!("{at}.keys: 레인 수 {} ≠ {lanes} → 기본 배치", keys.len()));
                    keys.clear();
                }
            }
            Some(v) => self.warn(format!("{at}.keys: 배열이 아님 ({v}) → 기본 배치")),
        }
        // 겹침·봇 키는 실행·화면과 같은 검사로 (리뷰 3-4 #3)
        let (clean, w) = crate::keys::sanitize(&keys);
        for m in w {
            self.warn(format!("{at}.keys: {m}"));
        }
        keys = clean;
        let visible_ms = num(&t, "visible_ms", 100.0, 5000.0, 600.0, &mut self.warnings, &at);
        let scroll_base = text(&t, "scroll_base", &SCROLL_BASES, "chart", &mut self.warnings, &at);
        KeyMode { keys, visible_ms, scroll_base }
    }

    pub fn set_keymode(&mut self, key: &str, k: &KeyMode) {
        let t = self.table_mut("keymode", Some(key));
        t.insert("keys".into(), Value::Array(k.keys.iter().map(|l| Value::Array(l.iter().map(|&v| Value::Integer(v as i64)).collect())).collect()));
        t.insert("visible_ms".into(), Value::Float(k.visible_ms));
        t.insert("scroll_base".into(), Value::String(k.scroll_base.clone()));
    }

    pub fn gauge(&mut self) -> String {
        let t = self.table("preset", Some("default"));
        text(&t, "gauge", &["normal", "easy", "assist", "hard", "exhard"], "normal", &mut self.warnings, "preset.default")
    }

    pub fn set_gauge(&mut self, g: &str) {
        self.table_mut("preset", Some("default")).insert("gauge".into(), Value::String(g.to_string()));
    }

    /// 변형 시드 (프리셋): −1 = 매번 무작위(원작처럼), 0 이상 = 고정 (같은 시드면 같은 변형)
    pub fn variant_seed(&mut self) -> i64 {
        let t = self.table("preset", Some("default"));
        num(&t, "variant_seed", -1.0, 1e15, -1.0, &mut self.warnings, "preset.default") as i64
    }

    pub fn set_variant_seed(&mut self, v: i64) {
        self.table_mut("preset", Some("default")).insert("variant_seed".into(), Value::Integer(v));
    }

    /// 쓰기: 임시 파일 → 바꾸기. 키 순서는 TOML 표 순서(정렬) — 다시 써도 같은 바이트.
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&self.path)
    }

    /// 다른 곳에 쓰기 (보정 실행에 저장 안 한 화면 값을 넘길 때). 임시 파일은 같은 폴더에 새로 만든 고유 파일.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        let s = toml::to_string_pretty(&self.doc).map_err(|e| e.to_string())?;
        let tmp = unique_sibling(path, "tmp");
        let r = (|| {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(s.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)
        })();
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_bad_values() {
        let dir = std::env::temp_dir().join(format!("cp3set-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.toml");
        std::fs::write(&p, "[global]\nvolume_master = 9.0\nunknown_key = \"keep\"\n[device.\"shared:X\"]\noffset_a_ms = -12.5\n").unwrap();
        let mut s = Settings::load(&p);
        let g = s.global();
        assert_eq!(g.volume_master, 1.0); // 범위 밖 → 기본값
        assert_eq!(s.warnings.len(), 1);
        assert_eq!(s.device("shared:X").offset_a_ms, -12.5);
        s.set_keymode("SC K1", &KeyMode { keys: vec![vec![0xA0], vec![b'Z' as u16, b'X' as u16]], visible_ms: 450.0, scroll_base: "mode".into() });
        s.set_global(&g);
        s.save().unwrap();
        let mut t = Settings::load(&p);
        assert_eq!(t.keymode("SC K1", 2).keys, vec![vec![0xA0], vec![b'Z' as u16, b'X' as u16]]);
        assert!(std::fs::read_to_string(&p).unwrap().contains("unknown_key")); // 모르는 키는 남는다
        let before = std::fs::read(&p).unwrap();
        Settings::load(&p).save().unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), before); // 다시 써도 같은 바이트
        std::fs::write(&p, "[global]\naudio = 123\n[device.\"shared:X\"]\nd_auto = \"false\"\n[keymode.\"SC K1\"]\nkeys = [[65, 999], [65, 124]]\n").unwrap();
        let mut t = Settings::load(&p);
        assert_eq!(t.global().audio, "shared");
        assert!(t.device("shared:X").d_auto);
        assert_eq!(t.keymode("SC K1", 2).keys, vec![vec![65], vec![]]); // 999 뺌, 겹친 A·봇 키 F13 뺌
        assert_eq!(t.warnings.len(), 5, "{:?}", t.warnings);
        std::fs::write(&p, "[[[깨짐").unwrap();
        let u = Settings::load(&p);
        assert_eq!(u.warnings.len(), 1);
        std::fs::write(&p, [0xFFu8, 0xFE, 0x00]).unwrap();
        let v = Settings::load(&p);
        assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
        let broken = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".broken-")).count();
        assert_eq!(broken, 2); // 두 번 다 고유한 이름으로 보존
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
