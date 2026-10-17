//! 설정 화면 (3-4, `cp3/설정창_계획.md` 6장): 선곡 화면에서 F1로 연다(플레이 중에는 열지 않음).
//! 키보드: Tab/Shift+Tab 탭, ↑↓ 항목, ←→ 값, Enter 실행·키 묶기, S 저장, Esc 닫기(저장 안 한 변경은 버림).
//! 탭 = 설정 체계 묶음: [규칙] [장비와 몸] [소리] [입력] [화면] [라이브러리] [엔진]. 항목마다 범위 꼬리표(전역·장치별·키 모드별·프리셋).
//! 화면이 들고 있는 값은 `Settings` 문서에 범위별로 되돌려 쓴다 — 장치(경로·장치 이름)나 레이아웃을 바꾸면 앞의 범위에 먼저 쓰고 새 범위를 읽는다.

use engine::calib::Calib;
use engine::judge::Rules;
use engine::runtime::LongRule;

use crate::calib_ui;
use crate::game::{gauge_of, rules_hash};
use crate::keys;
use crate::render::{DrawList, Rgba};
use crate::settings::{Device, Global, KeyMode, Settings, EMPTY_SOUNDS, SCROLL_BASES};

pub const TABS: [&str; 7] = ["규칙", "장비와 몸", "소리", "입력", "화면", "라이브러리", "엔진"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Close,
    /// 보정 실행 (true = 소리로 맞추기 A, false = 화면으로 맞추기 B)
    Calib(bool),
}

const GAUGES: [&str; 5] = ["normal", "easy", "assist", "hard", "exhard"];
const PATHS: [&str; 3] = ["shared", "exclusive", "asio"];

fn cycle<'a>(list: &[&'a str], cur: &str, d: i32) -> &'a str {
    let i = list.iter().position(|x| *x == cur).unwrap_or(0) as i32;
    list[((i + d).rem_euclid(list.len() as i32)) as usize]
}

pub struct Ui {
    pub set: Settings,
    pub tab: usize,
    pub row: usize,
    /// 키 묶는 중: 레인 번호
    pub binding: Option<usize>,
    pub gauge: String,
    /// 변형 시드 (−1 = 무작위)
    pub seed: i64,
    pub global: Global,
    dev_key: String,
    pub device: Device,
    layouts: Vec<String>,
    layout_i: usize,
    pub keymode: KeyMode,
    pub dirty: bool,
    pub msg: String,
    /// 보정 결과 (소리?, 새 값 ms, 설명) — 적용을 눌러야 값이 바뀐다
    pub pending: Option<(bool, f64, String)>,
}

impl Ui {
    /// `layout` = 지금 고른 차분의 레이아웃 열쇠 (있으면 입력·화면 탭이 그것부터)
    pub fn open(path: &std::path::Path, layout: Option<String>) -> Ui {
        let mut set = Settings::load(path);
        let global = set.global();
        let gauge = set.gauge();
        let seed = set.variant_seed();
        let dev_key = format!("{}:{}", global.audio, global.render);
        let device = set.device(&dev_key);
        let mut layouts: Vec<String> = keys::KNOWN.iter().map(|s| s.to_string()).collect();
        let layout_i = match layout {
            Some(l) => layouts.iter().position(|x| *x == l).unwrap_or_else(|| {
                layouts.insert(0, l);
                0
            }),
            None => 0,
        };
        let keymode = KeyMode { keys: vec![], visible_ms: 600.0, scroll_base: "chart".into() };
        let mut ui = Ui { set, tab: 0, row: 0, binding: None, gauge, seed, global, dev_key, device, layouts, layout_i, keymode, dirty: false, msg: String::new(), pending: None };
        ui.load_keymode();
        ui.msg = ui.set.warnings.join(" · ");
        ui
    }

    fn layout(&self) -> &str {
        &self.layouts[self.layout_i]
    }

    fn lanes(&self) -> Vec<&str> {
        self.layout().split(' ').collect()
    }

    fn load_keymode(&mut self) {
        let l = self.layout().to_string();
        self.keymode = self.set.keymode(&l, l.split(' ').count());
        if self.keymode.keys.is_empty() {
            self.keymode.keys = keys::default_keys(&l);
        }
    }

    /// 화면 값 → 설정 문서 (범위별)
    fn commit(&mut self) {
        self.set.set_gauge(&self.gauge);
        self.set.set_variant_seed(self.seed);
        self.set.set_global(&self.global);
        let dk = self.dev_key.clone();
        self.set.set_device(&dk, &self.device);
        let l = self.layout().to_string();
        let km = self.keymode.clone();
        self.set.set_keymode(&l, &km);
    }

    /// 경로·장치 이름이 바뀌면 장치별 값을 그 장치의 것으로
    fn switch_device(&mut self) {
        let dk = self.dev_key.clone();
        self.set.set_device(&dk, &self.device);
        self.dev_key = format!("{}:{}", self.global.audio, self.global.render);
        self.device = self.set.device(&self.dev_key);
    }

    pub fn save(&mut self) {
        self.commit();
        self.msg = match self.set.save() {
            Ok(()) => {
                self.dirty = false;
                format!("저장함: {}", self.set.path.display())
            }
            Err(e) => format!("저장 실패: {e}"),
        };
    }

    /// 보정 실행 전: 지금 화면 값(저장 안 한 것 포함)을 임시 설정 파일로 써서 실행에 넘긴다 (사용자 파일은 건드리지 않음)
    pub fn snapshot_to(&mut self, path: &std::path::Path) -> Result<(), String> {
        self.commit();
        self.set.save_to(path)
    }

    pub fn calib_done(&mut self, sound: bool, c: &Calib) {
        let line = calib_ui::describe(c, sound, self.device.offset_b_ms);
        self.pending = (c.used >= 8).then(|| (sound, calib_ui::proposal_ms(c, sound, self.device.offset_b_ms).round(), line.clone()));
        self.msg = line;
        self.tab = 1;
        self.row = 8;
    }

    pub fn rows(&self) -> usize {
        match self.tab {
            0 => 3,
            1 => 9,
            2 => 5,
            3 => 1 + self.lanes().len(),
            4 => 3,
            5 => 1,
            _ => 1,
        }
    }

    /// 키 하나 처리 (vk = 가상 키, shift = Shift 눌림)
    pub fn key(&mut self, vk: u32, shift: bool) -> Action {
        if let Some(lane) = self.binding {
            // 다음 키를 그 레인에 묶는다 (Esc = 취소, Backspace = 그 레인 비우기). 같은 키가 다른 레인에 있으면 거기서 뗀다.
            match vk {
                0x1B => {}
                0x08 => {
                    self.keymode.keys[lane].clear();
                    self.dirty = true;
                }
                v => {
                    let v = v as u16;
                    if (0..16).map(crate::bot::bot_key).any(|b| b == v) {
                        self.msg = format!("{}는 봇 키라 묶을 수 없음", keys::key_name(v));
                    } else {
                        let mut moved = None;
                        for (i, ks) in self.keymode.keys.iter_mut().enumerate() {
                            if i != lane && ks.contains(&v) {
                                ks.retain(|&k| k != v);
                                moved = Some(i);
                            }
                        }
                        if !self.keymode.keys[lane].contains(&v) {
                            self.keymode.keys[lane].push(v);
                        }
                        self.msg = match moved {
                            Some(i) => format!("{}를 레인 {}에서 떼어 옮김", keys::key_name(v), self.lanes()[i]),
                            None => String::new(),
                        };
                        self.dirty = true;
                    }
                }
            }
            self.binding = None;
            return Action::None;
        }
        let d = match vk {
            0x25 => -1,
            0x27 => 1,
            _ => 0,
        };
        match vk {
            0x09 => {
                self.tab = (self.tab + if shift { TABS.len() - 1 } else { 1 }) % TABS.len();
                self.row = 0;
            }
            0x26 => self.row = (self.row + self.rows() - 1) % self.rows().max(1),
            0x28 => self.row = (self.row + 1) % self.rows().max(1),
            0x1B => return Action::Close,
            0x53 => self.save(),
            0x0D => match (self.tab, self.row) {
                (1, 6) => return Action::Calib(true),
                (1, 7) => return Action::Calib(false),
                (1, 8) => {
                    if let Some((sound, v, _)) = self.pending.take() {
                        if sound {
                            self.device.offset_a_ms = v.clamp(-100.0, 100.0);
                        } else {
                            self.device.offset_b_ms = v.clamp(-200.0, 200.0);
                        }
                        self.dirty = true;
                        self.msg = format!("오프셋 {} = {v:+.0} ms 적용 (S로 저장)", if sound { "A" } else { "B" });
                    }
                }
                (3, r) if r >= 1 => self.binding = Some(r - 1),
                _ => {}
            },
            _ if d != 0 => {
                self.dirty = true;
                let f = d as f64;
                match (self.tab, self.row) {
                    (0, 0) => self.gauge = cycle(&GAUGES, &self.gauge, d).to_string(),
                    (0, 2) => self.seed = (self.seed + d as i64).max(-1),
                    (1, 0) => {
                        self.global.audio = cycle(&PATHS, &self.global.audio, d).to_string();
                        self.switch_device();
                    }
                    (1, 1) => self.device.d_auto = !self.device.d_auto,
                    (1, 2) => self.device.d_margin_ms = (self.device.d_margin_ms + f * 0.5).clamp(0.0, 50.0),
                    (1, 3) => self.device.d_manual_ms = (self.device.d_manual_ms + f).clamp(0.0, 200.0),
                    (1, 4) => self.device.offset_a_ms = (self.device.offset_a_ms + f).clamp(-100.0, 100.0),
                    (1, 5) => self.device.offset_b_ms = (self.device.offset_b_ms + f).clamp(-200.0, 200.0),
                    (2, 0) => self.global.volume_master = ((self.global.volume_master + f * 0.05) * 100.0).round().clamp(0.0, 200.0) / 100.0,
                    (2, 1) => self.global.volume_bgm = ((self.global.volume_bgm + f * 0.05) * 100.0).round().clamp(0.0, 200.0) / 100.0,
                    (2, 2) => self.global.volume_keys = ((self.global.volume_keys + f * 0.05) * 100.0).round().clamp(0.0, 200.0) / 100.0,
                    (2, 3) => self.global.late_drop_ms = (self.global.late_drop_ms + f * 10.0).clamp(0.0, 1000.0),
                    (2, 4) => self.global.empty_sound = cycle(&EMPTY_SOUNDS, &self.global.empty_sound, d).to_string(),
                    (3, 0) | (4, 0) => {
                        // 레이아웃 바꾸기: 지금 배치를 문서에 쓰고 다음 것을 읽는다 (변경 표시는 그대로)
                        let l = self.layout().to_string();
                        let km = self.keymode.clone();
                        self.set.set_keymode(&l, &km);
                        self.layout_i = (self.layout_i as i32 + d).rem_euclid(self.layouts.len() as i32) as usize;
                        self.load_keymode();
                    }
                    (4, 1) => self.keymode.visible_ms = (self.keymode.visible_ms + f * 25.0).clamp(100.0, 5000.0),
                    (4, 2) => self.keymode.scroll_base = cycle(&SCROLL_BASES, &self.keymode.scroll_base, d).to_string(),
                    _ => self.dirty = false,
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn draw(&self, list: &mut DrawList) {
        list.rect(0.0, 0.0, 1280.0, 720.0, Rgba(0.0, 0.0, 0.0, 0.88));
        for (i, t) in TABS.iter().enumerate() {
            let c = if i == self.tab { Rgba(1.0, 0.85, 0.4, 1.0) } else { Rgba(0.6, 0.6, 0.65, 1.0) };
            list.text(30.0 + i as f32 * 150.0, 20.0, 18.0, c, *t);
        }
        let pct = |v: f64| format!("{:.0}%", v * 100.0);
        let rows: Vec<(String, String, &str)> = match self.tab {
            0 => {
                let r = gauge_of(&self.gauge).map(Rules::iidx);
                let h = |lr| r.as_ref().map(|r| rules_hash(r, lr)[..8].to_string()).unwrap_or_default();
                vec![
                    ("게이지".into(), self.gauge.clone(), "프리셋"),
                    ("규칙 해시 (점수 비교 단위) LN · CN".into(), format!("{} · {}", h(LongRule::Ln), h(LongRule::Cn)), "판정 창·롱노트 규칙은 CP3 고정"),
                    ("변형 시드".into(), if self.seed < 0 { "무작위 (원작처럼 매번)".into() } else { format!("고정 {}", self.seed) }, "프리셋"),
                ]
            }
            1 => vec![
                ("오디오 경로".into(), self.global.audio.clone(), "전역"),
                ("D 자동".into(), if self.device.d_auto { "켜짐 (리드인에서 잼)".into() } else { "꺼짐 (수동 값)".into() }, "장치별"),
                ("D 자동 여유 (ms)".into(), format!("{:.1}", self.device.d_margin_ms), "장치별"),
                ("D 수동 (ms)".into(), format!("{:.0}", self.device.d_manual_ms), "장치별"),
                ("오프셋 A — 판정 + 키음 (ms)".into(), format!("{:+.0}", self.device.offset_a_ms), "장치별"),
                ("오프셋 B — 화면 (ms)".into(), format!("{:+.0}", self.device.offset_b_ms), "장치별"),
                ("소리로 맞추기 (A) ▶ Enter".into(), "클릭에 맞춰 누름, 40박".into(), ""),
                ("화면으로 맞추기 (B) ▶ Enter".into(), "판정선에 닿는 노트에 맞춰 누름".into(), ""),
                (
                    "보정 결과 적용 ▶ Enter".into(),
                    match &self.pending {
                        Some((s, v, _)) => format!("{} → {v:+.0} ms", if *s { "A" } else { "B" }),
                        None => "(보정을 먼저)".into(),
                    },
                    "",
                ),
            ],
            2 => vec![
                ("마스터".into(), pct(self.global.volume_master), "전역"),
                ("반주 (자동재생)".into(), pct(self.global.volume_bgm), "전역"),
                ("친 키음".into(), pct(self.global.volume_keys), "전역"),
                ("늦은 키음 한도 (ms)".into(), format!("{:.0}", self.global.late_drop_ms), "전역"),
                (
                    "빈 누름 소리".into(),
                    match self.global.empty_sound.as_str() {
                        "previous" => "직전 노트".into(),
                        "next" => "다음 노트".into(),
                        "none" => "없음".into(),
                        _ => "차분 규칙".into(),
                    },
                    "전역",
                ),
            ],
            3 => {
                let mut v = vec![("레이아웃 (←→)".into(), self.layout().to_string(), "키 모드별")];
                for (i, id) in self.lanes().iter().enumerate() {
                    let ks = self.keymode.keys.get(i).map(|k| k.iter().map(|&v| keys::key_name(v)).collect::<Vec<_>>().join(" / ")).unwrap_or_default();
                    v.push((format!("  레인 {id} ▶ Enter"), if self.binding == Some(i) { "… 키를 누르세요 (Backspace 비움, Esc 취소)".into() } else { ks }, ""));
                }
                v
            }
            4 => vec![
                ("레이아웃 (←→)".into(), self.layout().to_string(), "키 모드별"),
                ("노출 시간 (ms, 기준 스크롤에서)".into(), format!("{:.0}", self.keymode.visible_ms), "키 모드별"),
                (
                    "기준 스크롤".into(),
                    match self.keymode.scroll_base.as_str() {
                        "start" => "시작 속도".into(),
                        "mode" => "최빈 속도".into(),
                        "max" => "최고 속도".into(),
                        "min" => "최저 속도".into(),
                        _ => "곡 필드".into(),
                    },
                    "키 모드별",
                ),
            ],
            5 => vec![("라이브러리 폴더".into(), self.global.library.clone(), "전역 (색인은 trackc index)")],
            _ => vec![("미리보기 W · 디바운스 (⚑ 3-5 측정으로)".into(), "2초 · 150 ms".into(), "고정")],
        };
        for (i, (k, v, scope)) in rows.iter().enumerate() {
            let y = 70.0 + i as f32 * 30.0;
            if y > 620.0 {
                break;
            }
            let c = if i == self.row { Rgba(0.5, 0.9, 1.0, 1.0) } else { Rgba(0.8, 0.8, 0.85, 1.0) };
            list.text(50.0, y, 16.0, c, k.as_str());
            list.text(470.0, y, 16.0, c, v.as_str());
            list.text(960.0, y, 13.0, Rgba(0.55, 0.55, 0.6, 1.0), *scope);
        }
        if self.tab == 1 {
            list.text(50.0, 360.0, 13.0, Rgba(0.6, 0.65, 0.7, 1.0), format!("장치 범위: {}", self.dev_key));
            list.text(50.0, 380.0, 13.0, Rgba(0.6, 0.65, 0.7, 1.0), "누름 → 판정(A) → 키음 (D + A 뒤) · 반주는 D 뒤 · 노트 그림은 B만큼 앞당김");
        }
        list.text(30.0, 650.0, 14.0, Rgba(0.7, 0.7, 0.7, 1.0), "Tab 탭 · ↑↓ 항목 · ←→ 값 · Enter 실행/키 묶기 · S 저장 · Esc 닫기");
        if self.dirty {
            list.text(30.0, 676.0, 14.0, Rgba(1.0, 0.7, 0.4, 1.0), "저장 안 한 변경 있음 (S)");
        }
        list.text(250.0, 676.0, 14.0, Rgba(0.6, 0.95, 0.6, 1.0), self.msg.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_save_restore_by_scope() {
        let dir = std::env::temp_dir().join(format!("cp3ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.toml");
        let _ = std::fs::remove_file(&p);
        let mut u = Ui::open(&p, Some("SC K1 K2 K3 K4 K5 K6 K7".into()));
        // 장비와 몸: 오프셋 A +2, B −3
        u.key(0x09, false);
        u.row = 4;
        u.key(0x27, false);
        u.key(0x27, false);
        u.row = 5;
        for _ in 0..3 {
            u.key(0x25, false);
        }
        // 입력: 레인 K1(행 2)에 Q 묶기 → Z에 더해짐, 이어 S(레인 K2의 키)를 K1에 묶으면 K2에서 떨어짐
        u.tab = 3;
        u.row = 2;
        u.key(0x0D, false);
        u.key(b'Q' as u32, false);
        u.key(0x0D, false);
        u.key(b'S' as u32, false);
        assert_eq!(u.keymode.keys[1], vec![b'Z' as u16, b'Q' as u16, b'S' as u16]);
        assert!(u.keymode.keys[2].is_empty());
        // 봇 키는 묶지 않는다
        u.key(0x0D, false);
        u.key(0x7C, false);
        assert!(!u.keymode.keys[1].contains(&0x7C));
        // 다른 장치로 바꾼 뒤 A를 바꿔도 원래 장치 값은 남는다
        u.tab = 1;
        u.row = 0;
        u.key(0x27, false); // shared → exclusive
        u.row = 4;
        u.key(0x25, false);
        u.key(0x53, false); // S 저장
        assert!(!u.dirty, "{}", u.msg);
        let mut v = Settings::load(&p);
        assert_eq!(v.device("shared:Playback 1/2").offset_a_ms, 2.0);
        assert_eq!(v.device("shared:Playback 1/2").offset_b_ms, -3.0);
        assert_eq!(v.device("exclusive:Playback 1/2").offset_a_ms, -1.0);
        assert_eq!(v.global().audio, "exclusive");
        assert_eq!(v.keymode("SC K1 K2 K3 K4 K5 K6 K7", 8).keys[1], vec![b'Z' as u16, b'Q' as u16, b'S' as u16]);
        // 다시 열면 그대로
        let u2 = Ui::open(&p, Some("SC K1 K2 K3 K4 K5 K6 K7".into()));
        assert_eq!(u2.device.offset_a_ms, -1.0);
        assert!(u2.keymode.keys[2].is_empty());
        // 보정 결과는 적용을 눌러야 바뀐다
        let mut u3 = Ui::open(&p, None);
        let c = Calib { used: 36, dropped: 4, median: 20_000_000, mad: 0, suggest: -20_000_000, hist: vec![0; 121], hist_range_ms: 60 };
        u3.calib_done(true, &c);
        assert_eq!(u3.device.offset_a_ms, -1.0);
        u3.key(0x0D, false);
        assert_eq!(u3.device.offset_a_ms, -20.0);
    }
}
