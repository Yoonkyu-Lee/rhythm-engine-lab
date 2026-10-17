//! CP3 게임. Track 묶음을 여러 레인으로 플레이한다 (CP2 시험장에서 갈라져 나옴: 오디오·시계·입력·고장 주입·기록은 그대로).
//!
//!   cp3game select <라이브러리 폴더>                선곡 화면 (index.bin) + 미리보기 (select.rs)
//!   cp3game replay <실행 폴더>                     기록한 입력으로 다시 판정해 실시간 점수와 비교
//!   cp3game list <묶음.trk>                         차분 목록 (id, 이름, 레벨, 레인 수, 노트 수, 불완전 변환 여부)
//!   cp3game run <묶음.trk> [--chart id] [...]       플레이 (game.rs 머리 주석)
//!   cp3game run calib --calib sound|screen [...]   보정 (calib_ui.rs): 오프셋 A·B 제안
//!   cp3game inputtest [--window]                    입력 경로 진단 (SendInput → Raw Input)
//!   cp3game sounds <실행 폴더>                       그 실행의 차분 소리 표 (번호, 장치 프레임 수, 긴 소리?) — 싱크 분석용 (tools/analyze_sync.py)
//!   cp3game notes <실행 폴더>                        그 실행의 노트 소리 목록 (레인, 노트, 노트 시각, offset, 소리) — 싱크 분석용
//!   cp3game restream <실행 폴더> <출력.wav>          믹서 기록(mix_events.csv)으로 실제 출력 스트림을 다시 만든다 — 녹음 무결성 검사용
//! 설정: `--settings settings.toml` (settings.rs). 선곡 화면 F1 = 설정 화면 (settings_ui.rs).
//!   cp3game render <묶음.trk> [--chart id] [--rate 48000] [--from 초] <출력.f32>   정답 렌더(모든 노트를 제때 친 소리), --from이면 탐색 뒤부터

mod audio;
mod bench;
mod bot;
mod calib_ui;
mod capture;
mod envlog;
mod game;
mod graph;
mod input;
mod judging;
mod keys;
mod monitor;
mod render;
mod select;
mod settings;
mod settings_ui;

use engine::load::{fill_all, plan_chart_reuse, LoadOpts, SlotCache};
use engine::mixer::render_perfect;
use engine::runtime::Compiled;
use track::pack::{self, Bundle};

pub(crate) fn arg(args: &[String], key: &str, default: &str) -> String {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

pub(crate) fn flag(args: &[String], key: &str) -> bool {
    args.iter().any(|a| a == key)
}

/// 헤드룸 정책: 정답 렌더의 최댓값이 −1 dBFS를 넘으면 그만큼 줄인다 (곡마다 고정, 플레이 중 바꾸지 않음). CP2와 같음.
pub(crate) fn master_gain(c: &Compiled) -> (f32, f32) {
    let (pcm, _) = render_perfect(c, 4096);
    let peak = pcm.iter().fold(0f32, |m, &x| m.max(x.abs()));
    let limit = 10f32.powf(-1.0 / 20.0);
    (if peak > limit { limit / peak } else { 1.0 }, peak)
}

pub(crate) struct Loaded {
    pub compiled: Compiled,
    pub title: String,
    pub losses: Vec<String>,
}

fn info_title(b: &Bundle) -> String {
    b.section(pack::K_INFO, 0).and_then(|s| pack::R::new(s).str().ok()).unwrap_or_default()
}

/// 메모리 예산 (바이트, README 4-3 9): 계획의 최고 사용량 추정이 넘으면 시작하지 않는다. 3-5 측정: 라이브러리 871곡 중 추정 최대 952 MB
/// (실제 최고 사적 메모리 926 MB, BMS 한 폴더 PCM 711 MB) → 그 약 2배. 선곡 화면은 Track 저장소 합집합이 절반을 넘으면 비운다.
pub(crate) const BUDGET: u64 = 2_000_000_000;

/// 묶음을 열어 차분 하나를 장치 속도로 불러온다. `chart` = 차분 id (비면 첫 차분), `choice` = "0,1,…" 변형 선택.
/// `cache` = 같은 Track의 소리 저장소 (미리보기에서 넘겨받은 것): 준비된 슬롯은 다시 쓰고 모자란 것만 디코딩한다 (플레이 인계, README 4-3 7).
pub(crate) fn load_track(path: &str, chart: &str, choice: &str, rate: u32, random_seed: Option<u64>, cache: &mut SlotCache) -> Result<Loaded, String> {
    let b = Bundle::open(std::fs::read(path).map_err(|e| format!("{path}: {e}"))?)?;
    let list = engine::load::chart_list(&b)?;
    let idx = if chart.is_empty() { 0 } else { list.iter().position(|c| c.0 == chart).ok_or_else(|| format!("차분 없음: {chart} (있는 것: {})", list.iter().map(|c| c.0.as_str()).collect::<Vec<_>>().join(", ")))? };
    let choice: Vec<u16> = choice.split(',').filter(|s| !s.is_empty()).map(|s| s.trim().parse().map_err(|_| format!("--choice: {s}"))).collect::<Result<_, _>>()?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let plan = plan_chart_reuse(&b, idx, &LoadOpts { device_rate: rate, choice, threads, random_seed, budget: BUDGET }, cache)?;
    let reused = plan.compiled.chart.bank.data.len().saturating_sub(1 + plan.jobs.len());
    fill_all(&plan, threads)?;
    let compiled = std::sync::Arc::try_unwrap(plan.compiled).unwrap_or_else(|a| (*a).clone());
    if reused > 0 {
        println!("  미리보기에서 넘겨받은 소리 {reused}개 (새로 디코딩 {})", compiled.chart.bank.data.len() - 1 - reused);
    }
    let losses = pack::read_losses(&b)?.into_iter().find(|(id, _)| *id == list[idx].0).map(|(_, l)| l).unwrap_or_default();
    Ok(Loaded { compiled, title: info_title(&b), losses })
}

fn list(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.get(2).ok_or("묶음 파일")?;
    let b = Bundle::open(std::fs::read(path)?)?;
    let losses = pack::read_losses(&b)?;
    println!("{}", info_title(&b));
    for (i, c) in pack::read_charts(&b)?.iter().enumerate() {
        let sec = pack::decode_chart(b.section(pack::K_CHART, c.section as usize).ok_or("CHART 없음")?)?;
        let notes: usize = sec.lanes.iter().map(|l| l.notes.len()).sum();
        let lo = losses.iter().find(|(id, _)| *id == c.id).map(|(_, l)| format!("  불완전: {}", l.join("; "))).unwrap_or_default();
        println!("{i:3} {:24} {:12} lv {:4} 레인 {:2} 노트 {:5} 변형 {}{lo}", c.id, c.name, c.level, sec.lanes.len(), notes, sec.variants.len());
    }
    Ok(())
}

fn render(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.get(2).ok_or("묶음 파일")?;
    let out = args.last().ok_or("출력 파일")?;
    let rate: u32 = arg(args, "--rate", "48000").parse()?;
    let t0 = std::time::Instant::now();
    let l = load_track(path, &arg(args, "--chart", ""), &arg(args, "--choice", ""), rate, None, &mut SlotCache::new())?;
    let load_ms = t0.elapsed().as_secs_f64() * 1e3;
    let from_ns = (arg(args, "--from", "0").parse::<f64>()? * 1e9) as i64;
    let (pcm, log) = if from_ns > 0 { engine::mixer::render_from(&l.compiled, 4096, from_ns).ok_or("--from이 곡 끝 뒤")? } else { render_perfect(&l.compiled, 4096) };
    let bytes: Vec<u8> = pcm.iter().flat_map(|x| x.to_le_bytes()).collect();
    std::fs::write(out, bytes)?;
    // 발음 시작 목록 (차분 안 소리 번호, 시작 프레임): 독립 렌더와 하나하나 비교용
    let starts: String = log
        .iter()
        .filter_map(|e| match e {
            engine::mixer::MixEvent::Start { sound, start, .. } => Some(format!("{sound},{start}\n")),
            _ => None,
        })
        .collect();
    std::fs::write(format!("{out}.starts.csv"), starts)?;
    let starts = log.iter().filter(|e| matches!(e, engine::mixer::MixEvent::Start { .. })).count();
    println!("{} [{}] 불러오기 {load_ms:.0} ms, {} 프레임 @ {rate} Hz, 발음 {starts}, 메모리 {:.1} MB", l.title, l.compiled.chart.chart_id, pcm.len() / 2, l.compiled.chart.bank.bytes() as f64 / 1e6);
    Ok(())
}

/// 미리보기 준비 시간 (창·소리 없음): 곡 시각 `--at`에서 들을 준비(준비 집합)가 다 차기까지, 전체가 다 차기까지.
fn prevbench(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::AtomicU64;
    let path = args.get(2).ok_or("묶음 파일")?;
    let rate: u32 = arg(args, "--rate", "48000").parse()?;
    let at = (arg(args, "--at", "0").parse::<f64>()? * 1e9) as i64;
    let w = (arg(args, "--w", "2").parse::<f64>()? * 1e9) as i64;
    let threads: usize = arg(args, "--threads", "4").parse()?;
    let t0 = std::time::Instant::now();
    let b = Bundle::open(std::fs::read(path)?)?;
    let list = engine::load::chart_list(&b)?;
    let chart = arg(args, "--chart", "");
    let idx = if chart.is_empty() { 0 } else { list.iter().position(|c| c.0 == chart).ok_or("차분 없음")? };
    let t_open = t0.elapsed().as_secs_f64() * 1e3;
    let plan = engine::load::plan_chart(&b, idx, &LoadOpts { device_rate: rate, choice: vec![], threads, random_seed: None, budget: BUDGET })?;
    let t_plan = t0.elapsed().as_secs_f64() * 1e3;
    let store = engine::preview::Store::start(plan, at, threads, std::sync::Arc::new(AtomicU64::new(0)), 0);
    let mut t_ready = None;
    loop {
        if t_ready.is_none() && store.ready_at(at, w) {
            t_ready = Some(t0.elapsed().as_secs_f64() * 1e3);
        }
        if store.done.load(std::sync::atomic::Ordering::Acquire) + store.failed.load(std::sync::atomic::Ordering::Relaxed) >= store.total() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    let t_all = t0.elapsed().as_secs_f64() * 1e3;
    let c = store.compiled();
    println!(
        "{} [{}] @ {:.1} s, W {:.1} s, 스레드 {threads}: 열기 {t_open:.0} ms · 계획 {t_plan:.0} ms · 들을 준비 {:.0} ms · 전체 {t_all:.0} ms (슬롯 {}, 실패 {}, {:.1} MB)",
        info_title(&b),
        list[idx].0,
        at as f64 / 1e9,
        w as f64 / 1e9,
        t_ready.unwrap_or(t_all),
        store.total(),
        store.failed.load(std::sync::atomic::Ordering::Relaxed),
        c.chart.bank.bytes() as f64 / 1e6
    );
    store.join();
    Ok(())
}

/// 리플레이: 플레이 기록(judgments.csv의 입력 원본 행)을 엔진 판정으로 다시 돌려 meta.json의 점수와 비교한다.
/// 판정 스레드와 같이 키 반복과 같은 레인의 겹친 키는 거른다. 재동기화 무효(kind 7)가 있던 실행은 비교에서 그 수만큼 다를 수 있다.
fn replay(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(args.get(2).ok_or("실행 폴더 (runs/…)")?);
    let meta = std::fs::read_to_string(dir.join("meta.json"))?;
    let field = |k: &str| -> Option<String> {
        let i = meta.find(&format!("\"{k}\": "))? + k.len() + 4;
        let rest = &meta[i..];
        let end = rest.find([',', '\n', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim().trim_matches('"').to_string())
    };
    let trk = field("track").ok_or("meta.json에 track 없음")?;
    let chart = field("chart").unwrap_or_default();
    let rate: u32 = field("rate").and_then(|r| r.parse().ok()).unwrap_or(48000);
    let gauge = game::gauge_of(&field("gauge").unwrap_or("Normal".into()).to_lowercase().replace("assisteasy", "assist"))?;
    let choice = meta.find("\"variant_choice\": [").and_then(|i| { let r = &meta[i + 19..]; r.find(']').map(|j| r[..j].replace(' ', "")) }).unwrap_or_default();
    let l = load_track(&trk, &chart, &choice, rate, None, &mut SlotCache::new())?;
    let c = &l.compiled.chart;
    let csv = std::fs::read_to_string(dir.join("judgments.csv"))?;
    let mut inputs = Vec::new();
    let mut down_keys: std::collections::HashMap<u16, bool> = Default::default();
    let mut held = vec![0u32; c.lanes.len()];
    // 실시간의 마지막 확정 시각 = 판정 스레드가 끝날 때 남긴 마지막 watermark (kind 11). 없으면(옛 기록) 가장 늦은 곡 시각
    let until = csv
        .lines()
        .skip(1)
        .find(|l| l.starts_with("11,"))
        .and_then(|l| l.split(',').nth(5)?.parse::<i64>().ok())
        .or_else(|| csv.lines().skip(1).filter_map(|l| l.split(',').nth(5)?.parse::<i64>().ok()).max())
        .unwrap_or(c.end);
    let resync_void = field("resync_void").map_or(true, |v| v == "true");
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] != "8" {
            continue;
        }
        let (lane, down, t, vkey, state): (usize, bool, i64, u16, u8) = (f[1].parse()?, f[3] == "1", f[5].parse()?, f[8].parse()?, f[10].parse()?);
        let was = down_keys.insert(vkey, down).unwrap_or(false);
        if was == down {
            continue; // 키 반복
        }
        if down {
            held[lane] += 1;
            if held[lane] > 1 {
                continue;
            }
        } else {
            held[lane] = held[lane].saturating_sub(1);
            if held[lane] > 0 {
                continue;
            }
        }
        inputs.push((lane, t, down, resync_void && state == 2));
    }
    let rules = engine::judge::Rules::iidx(gauge);
    // 규칙 해시 (3-4): 기록과 다르면 같은 규칙의 점수가 아니므로 비교에서 뺀다 (옛 기록은 해시가 없음 → 비교)
    let rh = game::rules_hash(&rules, c.long_rule, resync_void);
    if let Some(m) = field("rules_hash") {
        if m != rh {
            println!("{} [{}]: 규칙 해시 다름 (기록 {}, 지금 {}) → 비교에서 뺌", l.title, chart, &m[..8.min(m.len())], &rh[..8]);
            return Ok(());
        }
    }
    let s = engine::judge::replay(c, &rules, until, &inputs);
    let live = |k: &str| -> String {
        let i = meta.find("\"score\"").map(|i| &meta[i..]).unwrap_or("");
        i.find(&format!("\"{k}\": ")).map(|j| i[j + k.len() + 4..].split([',', '}']).next().unwrap_or("").trim().to_string()).unwrap_or_default()
    };
    let mine = [("ex", s.ex().to_string()), ("pgreat", s.pgreat.to_string()), ("great", s.great.to_string()), ("good", s.good.to_string()), ("bad", s.bad.to_string()),
                ("poor", s.poor.to_string()), ("empty_poor", s.empty_poor.to_string()), ("mines", s.mines.to_string()), ("max_combo", s.max_combo.to_string()),
                ("fast", s.fast.to_string()), ("slow", s.slow.to_string()), ("gauge", s.gauge.value.to_string()), ("cleared", s.gauge.cleared().to_string())];
    let mut same = true;
    for (k, v) in &mine {
        let lv = live(k);
        same &= lv == *v;
        println!("  {k:10} 실시간 {lv:>6}  리플레이 {v:>6}{}", if lv == *v { "" } else { "  ← 다름" });
    }
    println!("{} [{}] 규칙 {} 입력 {}개: {}", l.title, chart, &rh[..8], inputs.len(), if same { "리플레이 = 실시간" } else { "다름" });
    if !same {
        return Err("리플레이 ≠ 실시간".into());
    }
    Ok(())
}

/// 실행 폴더의 meta.json으로 그 실행과 같은 차분(변형 선택 포함)을 장치 속도로 다시 불러온다.
pub(crate) fn load_run(dir: &std::path::Path) -> Result<(Loaded, String), Box<dyn std::error::Error>> {
    let meta = std::fs::read_to_string(dir.join("meta.json"))?;
    let field = |k: &str| -> Option<String> {
        let i = meta.find(&format!("\"{k}\": "))? + k.len() + 4;
        let rest = &meta[i..];
        let end = rest.find([',', '\n', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim().trim_matches('"').to_string())
    };
    let trk = field("track").ok_or("meta.json에 track 없음")?;
    if trk == "calib" {
        return Err("보정 실행은 대상 아님".into());
    }
    let rate: u32 = field("rate").and_then(|r| r.parse().ok()).ok_or("meta.json에 rate 없음")?;
    let choice = meta.find("\"variant_choice\": [").and_then(|i| { let r = &meta[i + 19..]; r.find(']').map(|j| r[..j].replace(' ', "")) }).unwrap_or_default();
    let l = load_track(&trk, &field("chart").unwrap_or_default(), &choice, rate, None, &mut SlotCache::new())?;
    Ok((l, meta))
}

fn sounds(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (l, _) = load_run(std::path::Path::new(args.get(2).ok_or("실행 폴더")?))?;
    println!("sound,frames,long");
    for (i, s) in l.compiled.chart.bank.sounds.iter().enumerate() {
        println!("{i},{},{}", s.frames, (s.kind == engine::runtime::SoundKind::Long) as u8);
    }
    Ok(())
}

/// 믹서는 결정적이라 "어느 소리를, 어느 프레임에, 어느 블록부터, 어떤 이득으로" 냈는지만 알면 출력을 그대로 다시 만든다 (CP2 restream과 같음).
/// 보이스 이득 = 요청 이득 × 소리 표 이득 × 음량(반주/친 키음) × 마스터. 끊기(cut)는 그 프레임에서 끝.
fn restream(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use engine::runtime::Pcm2;
    use std::io::Write;
    let dir = std::path::PathBuf::from(args.get(2).ok_or("실행 폴더")?);
    let out_path = args.get(3).ok_or("출력.wav")?;
    let (l, meta) = load_run(&dir)?;
    let num = |k: &str| -> f32 { meta.find(&format!("\"{k}\": ")).and_then(|i| meta[i + k.len() + 4..].split([',', '\n', '}']).next()?.trim().parse().ok()).unwrap_or(1.0) };
    let vols: Vec<f32> = meta.find("\"volumes\": [").and_then(|i| { let r = &meta[i + 12..]; r.find(']').map(|j| r[..j].split(',').filter_map(|x| x.trim().parse().ok()).collect()) }).unwrap_or_default();
    let (vbgm, vkeys) = (vols.get(1).copied().unwrap_or(1.0), vols.get(2).copied().unwrap_or(1.0));
    let gain = num("master_gain");
    let rate = num("rate") as u32;
    let bank = &l.compiled.chart.bank;
    // (소리, 시작, 블록, 이득, 끝)
    let mut starts: Vec<(u32, i64, i64, f32, i64)> = Vec::new();
    for line in std::fs::read_to_string(dir.join("mix_events.csv"))?.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        match f[0] {
            "start" => {
                let (sound, start, block, g, player): (u32, i64, i64, f32, bool) = (f[1].parse()?, f[3].parse()?, f[4].parse()?, f[7].parse()?, f[6] == "1");
                let snd = bank.sounds[sound as usize];
                starts.push((sound, start, block, g * snd.gain * if player { vkeys } else { vbgm }, start + snd.frames as i64));
            }
            "cut" => {
                let (sound, at): (u32, i64) = (f[1].parse()?, f[2].parse()?);
                for v in starts.iter_mut().filter(|v| v.0 == sound && v.4 > at) {
                    v.4 = at;
                }
            }
            _ => {}
        }
    }
    let end = starts.iter().map(|v| v.4).max().unwrap_or(0).max(0) as usize;
    let mut out = vec![0f32; end * 2];
    for &(s, start, block, g, stop) in &starts {
        let Some(pcm) = bank.data[bank.sounds[s as usize].data as usize].get() else { continue };
        let lo = start.max(block).max(0);
        let n = match pcm {
            Pcm2::F32(x) => x.len() / 2,
            Pcm2::I16(x) => x.len() / 2,
        } as i64;
        for f in lo..stop.min(start + n) {
            let k = (f - start) as usize * 2;
            let (a, b) = match pcm {
                Pcm2::F32(x) => (x[k], x[k + 1]),
                Pcm2::I16(x) => (x[k] as f32 / 32768.0, x[k + 1] as f32 / 32768.0),
            };
            out[f as usize * 2] += a * g;
            out[f as usize * 2 + 1] += b * g;
        }
    }
    out.iter_mut().for_each(|v| *v *= gain);
    let data = (out.len() * 4) as u32;
    let mut w = std::io::BufWriter::new(std::fs::File::create(out_path)?);
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&3u16.to_le_bytes())?;
    w.write_all(&2u16.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * 8).to_le_bytes())?;
    w.write_all(&8u16.to_le_bytes())?;
    w.write_all(&32u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data.to_le_bytes())?;
    for v in &out {
        w.write_all(&v.to_le_bytes())?;
    }
    println!("{out_path}: 발음 {} 개, {:.1}초", starts.len(), end as f64 / rate as f64);
    Ok(())
}

/// 입력 경로 진단: Raw Input 스레드를 띄우고 SendInput으로 F24를 5번 눌렀다 뗀 뒤, 받은 이벤트 수와 앞 창을 출력한다.
fn inputtest(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let _w = if flag(args, "--window") { Some(game::create_window(windows::core::w!("입력 시험"))?) } else { None };
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_F24};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let (tx, mut rx) = engine::ring::ring::<synchronizer::input::KeyEvent>(256);
    let input = input::Input::spawn(tx, false)?;
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mut sent = 0;
    for _ in 0..5 {
        for up in [false, true] {
            let ki = KEYBDINPUT { wVk: VK_F24, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() };
            sent += unsafe { SendInput(&[INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }], std::mem::size_of::<INPUT>() as i32) };
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut got = 0;
    while let Some(e) = rx.pop() {
        got += 1;
        println!("  받음 vkey {:#x} down {}", e.vkey, e.down);
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    println!("SendInput {sent}/10 성공, Raw Input 받음 {got}/10, 앞 창 pid {pid} (이 프로세스 {})", std::process::id());
    input.stop();
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("run") => game::run(&args).map(|_| ()),
        Some("select") => select::run(&args),
        Some("list") => list(&args),
        Some("render") => render(&args),
        Some("prevbench") => prevbench(&args),
        Some("replay") => replay(&args),
        Some("inputtest") => inputtest(&args),
        Some("sounds") => sounds(&args),
        Some("restream") => restream(&args),
        Some("loadbench") => bench::loadbench(&args),
        Some("prevsweep") => bench::prevsweep(&args),
        Some("memtest") => bench::memtest(&args),
        Some("partcheck") => bench::partcheck(&args),
        Some("notes") => bench::notes(&args),
        _ => {
            eprintln!("사용법: cp3game list <묶음.trk> | cp3game run <묶음.trk> [--chart id] [--bot] … | cp3game render <묶음.trk> [--chart id] <출력.f32>");
            Ok(())
        }
    };
    if let Err(e) = r {
        eprintln!("오류: {e}");
        std::process::exit(1);
    }
}
