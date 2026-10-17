//! 레이아웃마다 기본 키 배치. 사용자 배치는 설정(`settings.toml`의 `[keymode."<레인 id 서명>"]`)이 덮는다(3-4). 레인 id 순서로 레이아웃을 알아본다 — 하드코딩은 이 표 하나뿐이고,
//! 표에 없는 레이아웃은 자판 한 줄 순서로 채운다. 왼쪽·오른쪽 Shift는 입력 스레드가 구분한다(0xA0 / 0xA1).
//! 봇 키(`bot::bot_key`)는 레인마다 덧붙인다.

use engine::runtime::Lane;

const LSHIFT: u16 = 0xA0;
const RSHIFT: u16 = 0xA1;
const SPACE: u16 = 0x20;
const COMMA: u16 = 0xBC;
const PERIOD: u16 = 0xBE;
const SEMI: u16 = 0xBA;
const SLASH: u16 = 0xBF;
const QUOTE: u16 = 0xDE;

fn k(c: char) -> u16 {
    c as u16
}

fn table(ids: &str) -> Option<Vec<u16>> {
    Some(match ids {
        // DJMAX
        "SL K1 K2 K3 K4 SR" => vec![LSHIFT, k('D'), k('F'), k('J'), k('K'), RSHIFT],
        "SL K1 K2 K3 K4 K5 SR" => vec![LSHIFT, k('D'), k('F'), SPACE, k('J'), k('K'), RSHIFT],
        "SL K1 K2 K3 K4 K5 K6 SR" => vec![LSHIFT, k('S'), k('D'), k('F'), k('J'), k('K'), k('L'), RSHIFT],
        "SL K1 K2 K3 L1 R1 K4 K5 K6 SR" => vec![LSHIFT, k('S'), k('D'), k('F'), k('V'), k('N'), k('J'), k('K'), k('L'), RSHIFT],
        // IIDX·BMS
        "SC K1 K2 K3 K4 K5" => vec![LSHIFT, k('Z'), k('S'), k('X'), k('D'), k('C')],
        "SC K1 K2 K3 K4 K5 K6 K7" => vec![LSHIFT, k('Z'), k('S'), k('X'), k('D'), k('C'), k('F'), k('V')],
        "SC1 K1 K2 K3 K4 K5 K6 K7 K8 K9 K10 SC2" => vec![LSHIFT, k('Z'), k('S'), k('X'), k('D'), k('C'), k('M'), k('K'), COMMA, k('L'), PERIOD, RSHIFT],
        "SC1 K1 K2 K3 K4 K5 K6 K7 K8 K9 K10 K11 K12 K13 K14 SC2" => vec![
            LSHIFT, k('Z'), k('S'), k('X'), k('D'), k('C'), k('F'), k('V'), k('M'), k('K'), COMMA, k('L'), PERIOD, SEMI, SLASH, RSHIFT,
        ],
        // 팝픈식 9키
        "K1 K2 K3 K4 K5 K6 K7 K8 K9" => vec![k('A'), k('S'), k('D'), k('F'), SPACE, k('J'), k('K'), k('L'), SEMI],
        _ => return None,
    })
}

/// 표에 있는 레이아웃들 (설정 화면의 입력 탭에서 고른다)
pub const KNOWN: [&str; 9] = [
    "SC K1 K2 K3 K4 K5 K6 K7",
    "SC1 K1 K2 K3 K4 K5 K6 K7 K8 K9 K10 K11 K12 K13 K14 SC2",
    "SC K1 K2 K3 K4 K5",
    "SC1 K1 K2 K3 K4 K5 K6 K7 K8 K9 K10 SC2",
    "SL K1 K2 K3 K4 SR",
    "SL K1 K2 K3 K4 K5 SR",
    "SL K1 K2 K3 K4 K5 K6 SR",
    "SL K1 K2 K3 L1 R1 K4 K5 K6 SR",
    "K1 K2 K3 K4 K5 K6 K7 K8 K9",
];

/// 레이아웃 열쇠 = 레인 id를 공백으로 이은 것 (설정의 키 모드 범위)
pub fn layout_key(lanes: &[Lane]) -> String {
    lanes.iter().map(|l| l.id.as_str()).collect::<Vec<_>>().join(" ")
}

/// 레이아웃 열쇠의 기본 사람 키 (레인마다 하나)
pub fn default_keys(ids: &str) -> Vec<Vec<u16>> {
    let n = ids.split(' ').filter(|x| !x.is_empty()).count();
    let row = [k('A'), k('S'), k('D'), k('F'), k('G'), k('H'), k('J'), k('K'), k('L'), SEMI, QUOTE, k('Q'), k('W'), k('E'), k('R'), k('T'), k('Y'), k('U'), k('I'), k('O'), k('P')];
    table(ids).unwrap_or_else(|| row.iter().copied().take(n).collect()).into_iter().map(|v| vec![v]).collect()
}

/// 봇 키인가 (레인 16개분: F13~F24, 숫자패드 0~3)
pub fn is_bot_key(v: u16) -> bool {
    (0..16).any(|i| crate::bot::bot_key(i) == v)
}

/// 사람 키 배치 검사 (설정 읽기·설정 화면·실행이 함께 쓴다, 리뷰 3-4 #3): 봇 키는 빼고, 여러 레인에 묶인 키는 앞 레인에만 남긴다.
/// 레인을 일부러 비운 것(`[]`)은 그대로 둔다. 돌려주는 값 = (고친 배치, 고친 내용).
pub fn sanitize(keys: &[Vec<u16>]) -> (Vec<Vec<u16>>, Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let mut w = Vec::new();
    let out = keys
        .iter()
        .enumerate()
        .map(|(i, ks)| {
            let mut v = Vec::new();
            for &k in ks {
                if is_bot_key(k) {
                    w.push(format!("레인 {i}의 {}는 봇 키 → 뺌", key_name(k)));
                } else if !seen.insert(k) {
                    w.push(format!("레인 {i}의 {}는 앞 레인에 이미 있음 → 뺌", key_name(k)));
                } else {
                    v.push(k);
                }
            }
            v
        })
        .collect();
    (out, w)
}

/// (가상 키, 레인 번호) 목록: 사람 키 + 봇 키. `custom`이 레인 수만큼 있으면 그것을(검사를 거쳐), 아니면 기본 배치를 쓴다.
pub fn keymap(lanes: &[Lane], custom: &[Vec<u16>]) -> Vec<(u16, usize)> {
    let human = if custom.len() == lanes.len() { sanitize(custom).0 } else { default_keys(&layout_key(lanes)) };
    let mut out: Vec<(u16, usize)> = human.iter().enumerate().flat_map(|(i, ks)| ks.iter().map(move |&v| (v, i))).collect();
    out.extend((0..lanes.len()).map(|i| (crate::bot::bot_key(i), i)));
    out
}

pub fn key_name(v: u16) -> String {
    match v {
        LSHIFT => "LShift".into(),
        RSHIFT => "RShift".into(),
        SPACE => "Space".into(),
        COMMA => ",".into(),
        PERIOD => ".".into(),
        SEMI => ";".into(),
        SLASH => "/".into(),
        QUOTE => "'".into(),
        0x30..=0x39 | 0x41..=0x5A => (v as u8 as char).to_string(),
        0xA2 => "LCtrl".into(),
        0xA3 => "RCtrl".into(),
        0xA4 => "LAlt".into(),
        0xA5 => "RAlt".into(),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        0x60..=0x69 => format!("Num{}", v - 0x60),
        0x70..=0x87 => format!("F{}", v - 0x6F),
        _ => format!("{v:#x}"),
    }
}

/// 레인마다 사람 키 이름 (화면 표시용, 여러 개면 '/'로).
pub fn keymap_names(map: &[(u16, usize)], n: usize) -> Vec<String> {
    let bots: Vec<u16> = (0..n).map(crate::bot::bot_key).collect();
    (0..n).map(|i| map.iter().filter(|(v, l)| *l == i && !bots.contains(v)).map(|(v, _)| key_name(*v)).collect::<Vec<_>>().join("/")).collect()
}
