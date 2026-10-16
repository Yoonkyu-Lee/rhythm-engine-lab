//! 믹서: 실행 데이터를 출력 스트림 프레임에 섞는다. 오프라인 렌더와 실시간 오디오 콜백이 같은 코드를 쓴다.
//!
//! - 콘텐츠 대응: 곡 시간 0이 놓이는 스트림 프레임 `origin`. 곡 시각 t의 소리는 프레임 `origin + ns_to_frame(t)`에서 시작한다.
//! - 자동재생: `auto`를 커서 하나로 앞에서부터 훑는다. 블록이 끝나기 전에 시작하는 이벤트를 보이스로 올린다.
//! - 플레이어 키음: `play_at`으로 받은 요청(목표 프레임)을 같은 방식으로 올린다 (실시간에서는 판정 스레드 → SPSC → 여기).
//! - 보이스는 고정 크기 풀(보이스 예산)이고, **들어온 순서를 유지**한다. 그래서 더하는 순서가 블록 크기와 무관하고,
//!   늦은 일이 없으면 블록을 어떻게 나눠 채워도 결과가 비트 단위로 같다.
//! - 늦은 소리(시작 프레임이 이미 지남): 긴 소리와 짧은 소리 각각 정책을 따른다.
//!
//! 콜백 안에서 할당·잠금·파일 작업을 하지 않는다 (보이스 풀과 로그 버퍼는 미리 잡는다).

use crate::runtime::{ns_to_frame, Compiled, SoundKind};

/// 늦게 시작하는 소리의 처리.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Late {
    /// 늦은 만큼 앞부분을 잘라 내고 남은 부분을 원래 자리에 맞춰 낸다.
    CutFront,
    /// 늦어도 처음부터 낸다 (통째로 밀림).
    PlayWhole,
    /// 내지 않는다.
    Drop,
}

/// 늦은 소리의 처리를 출처별로 정한다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatePolicy {
    /// 플레이어가 친 키음 (설계안 8-3 최소 지연 모드: 가장 이른 안전한 프레임에 처음부터 = PlayWhole).
    /// 앞부분을 자르면 소리의 어택이 사라진다 (CP2 2-2에서 2초 넘는 키음이 잘리는 것으로 발견).
    pub player: Late,
    /// 자동재생의 짧은 소리.
    pub short: Late,
    /// 자동재생의 긴 소리 (반주): 박자를 지키도록 자르기 (CP1 3-3).
    pub long: Late,
    /// "처음부터(PlayWhole)"인 소리도 이 프레임 수보다 더 늦으면 버린다 (i64::MAX면 끝없이 민다).
    pub drop_after: i64,
}

impl LatePolicy {
    /// 자동재생 짧은 소리는 CP2 2-5에서 정한다 (임시: 처음부터).
    pub const DEFAULT: LatePolicy = LatePolicy { player: Late::PlayWhole, short: Late::PlayWhole, long: Late::CutFront, drop_after: i64::MAX };
}

/// 보이스 하나. `start`는 소리의 0번 샘플이 놓인 스트림 프레임 (자르기면 블록보다 앞일 수 있음).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Voice {
    sound: u32,
    start: i64,
    gain: f32,
    end: i64,
}

/// 믹서에서 일어난 일 (개발자 로그용). 콜백에서는 미리 잡은 링에 넣고, 다른 스레드가 꺼내 기록한다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixEvent {
    /// 소리를 올렸다. `late` = 블록 시작 − 예정 시작 (프레임, 0 이하면 제때).
    Start { sound: u32, due: i64, start: i64, block: i64, late: i64, player: bool, gain: f32 },
    /// 늦어서 버렸다.
    Dropped { sound: u32, due: i64, block: i64, player: bool },
    /// 보이스 풀이 가득 차서 올리지 못했다.
    Overflow { sound: u32, due: i64, player: bool },
    /// 같은 소리를 다시 울려서 앞 소리를 프레임 `at`에서 끊었다 (재트리거 끊기 정책).
    Cut { sound: u32, at: i64 },
}

/// "지금 울려": 예정 프레임 대신 이 값을 주면 요청을 받은 블록의 맨 앞에서 시작한다 (늦음으로 치지 않음).
/// 프레임에서 소리를 내는 구동 방식(BMS·DJMAX·IIDX식)의 즉시 재생이다.
pub const NOW: i64 = i64::MIN;

pub struct Mixer<'a> {
    c: &'a Compiled,
    origin: i64,
    late: LatePolicy,
    cursor: usize,
    voices: Vec<Voice>,
    /// 요청 대기열 (목표 프레임 또는 NOW, 소리, 이득, 플레이어?). 실시간에서는 SPSC 링이 이 자리를 채운다.
    pending: Vec<(i64, u32, f32, bool)>,
    /// 자동재생을 믹서가 실행 데이터에서 직접 훑는가 (우리 방식). 끄면 프레임 쪽 로직이 요청으로 울린다.
    auto_from_chart: bool,
    /// 같은 소리를 다시 울리면 앞 소리를 끊는가 (P9: BMS 구동기 방식).
    cut_same: bool,
    events: Vec<MixEvent>,
    events_cap: usize,
    pub overflows: u64,
    pub events_lost: u64,
}

impl<'a> Mixer<'a> {
    /// `origin`: 곡 시간 0이 놓이는 스트림 프레임. `events_cap`: 블록 사이 로그 버퍼 크기.
    pub fn new(c: &'a Compiled, origin: i64, late: LatePolicy, events_cap: usize) -> Self {
        Mixer {
            c,
            origin,
            late,
            cursor: 0,
            voices: Vec::with_capacity(c.voice_budget as usize),
            pending: Vec::with_capacity(256),
            auto_from_chart: true,
            cut_same: false,
            events: Vec::with_capacity(events_cap),
            events_cap,
            overflows: 0,
            events_lost: 0,
        }
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// 자동재생을 실행 데이터에서 직접 훑을지 (끄면 `play_sound`로만 울린다).
    pub fn set_auto_from_chart(&mut self, on: bool) {
        self.auto_from_chart = on;
    }

    /// 같은 소리를 다시 울릴 때 앞 소리를 끊을지.
    pub fn set_cut_same(&mut self, on: bool) {
        self.cut_same = on;
    }

    /// 플레이어 키음 요청: 노트 `note`(레인 `lane`)의 소리들을 스트림 프레임 `frame`(또는 NOW)에서 낸다.
    pub fn play_note(&mut self, lane: usize, note: usize, frame: i64) {
        let n = self.c.chart.lanes[lane].notes[note];
        for s in &self.c.chart.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize] {
            if self.pending.len() < self.pending.capacity() {
                self.pending.push((frame, s.sound, s.gain, true));
            } else {
                self.overflows += 1;
            }
        }
    }

    /// 소리 하나를 프레임 `frame`(또는 NOW)에서 낸다. `player`면 플레이어 키음의 늦음 정책을 따른다.
    pub fn play_sound(&mut self, sound: u32, gain: f32, frame: i64, player: bool) {
        if self.pending.len() < self.pending.capacity() {
            self.pending.push((frame, sound, gain, player));
        } else {
            self.overflows += 1;
        }
    }

    /// 지난 블록 이후의 믹서 사건을 꺼낸다 (콜백 밖에서 부른다).
    pub fn drain_events(&mut self, into: &mut Vec<MixEvent>) {
        into.extend(self.events.drain(..));
    }

    /// 지난 블록 이후의 믹서 사건을 하나씩 넘긴다 (할당 없음. 실시간 콜백에서 SPSC 링으로 보낼 때).
    pub fn drain_each(&mut self, mut f: impl FnMut(MixEvent)) {
        for e in self.events.drain(..) {
            f(e);
        }
    }

    /// 지금까지 자동재생 커서가 지나간 이벤트 수.
    pub fn auto_cursor(&self) -> usize {
        self.cursor
    }

    fn log(&mut self, e: MixEvent) {
        if self.events.len() < self.events_cap {
            self.events.push(e);
        } else {
            self.events_lost += 1;
        }
    }

    fn admit(&mut self, sound: u32, due: i64, gain: f32, block: i64, player: bool) {
        let frames = self.c.chart.bank.sounds[sound as usize].frames as i64;
        let late = block - due;
        let start = if late <= 0 {
            due
        } else {
            let p = match (player, self.c.chart.bank.sounds[sound as usize].kind) {
                (true, _) => self.late.player,
                (false, SoundKind::Long) => self.late.long,
                (false, SoundKind::Short) => self.late.short,
            };
            match p {
                Late::CutFront => due,
                Late::PlayWhole if late <= self.late.drop_after => block,
                Late::PlayWhole | Late::Drop => {
                    self.log(MixEvent::Dropped { sound, due, block, player });
                    return;
                }
            }
        };
        if start + frames <= block {
            // 자르고 나니 남은 것이 없다
            self.log(MixEvent::Dropped { sound, due, block, player });
            return;
        }
        if self.cut_same {
            let at = start.max(block);
            let mut cut = false;
            for v in self.voices.iter_mut().filter(|v| v.sound == sound && v.end > at) {
                v.end = at;
                cut = true;
            }
            if cut {
                self.log(MixEvent::Cut { sound, at });
            }
        }
        if self.voices.len() >= self.voices.capacity() {
            self.overflows += 1;
            self.log(MixEvent::Overflow { sound, due, player });
            return;
        }
        self.voices.push(Voice { sound, start, gain, end: start + frames });
        self.log(MixEvent::Start { sound, due, start, block, late, player, gain });
    }

    /// 스트림 프레임 [block, block + n)을 채운다. `out`은 출력 채널 수(`out_ch`)만큼 끼워 넣은 버퍼이고 0으로 초기화돼 있어야 한다.
    /// 뱅크가 2채널이면 그대로, 출력이 더 많으면 앞 두 채널에만 쓴다.
    pub fn fill(&mut self, out: &mut [f32], out_ch: usize, block: i64, n: usize) {
        let end = block + n as i64;
        let rate = self.c.chart.bank.rate;
        // 1)·2) 이 블록 안에서 시작하는(또는 이미 늦은) 자동재생 이벤트와 플레이어 키음 요청을 예정 프레임 순으로 올린다
        //      (같은 프레임이면 자동재생 먼저). 출처와 무관하게 순서를 정해야 블록 크기가 결과를 바꾸지 않는다.
        // NOW 요청은 이 블록 맨 앞으로
        for p in self.pending.iter_mut() {
            if p.0 == NOW {
                p.0 = block;
            }
        }
        loop {
            let auto_due = (self.auto_from_chart && self.cursor < self.c.chart.auto.len())
                .then(|| self.origin + ns_to_frame(self.c.chart.auto[self.cursor].t, rate))
                .filter(|&d| d < end);
            let mut best: Option<(usize, i64)> = None;
            for (i, &(d, _, _, _)) in self.pending.iter().enumerate() {
                if d < end && best.map_or(true, |(_, b)| d < b) {
                    best = Some((i, d));
                }
            }
            match (auto_due, best) {
                (Some(a), b) if b.map_or(true, |(_, p)| a <= p) => {
                    let e = self.c.chart.auto[self.cursor];
                    self.cursor += 1;
                    self.admit(e.sound, a, e.gain, block, false);
                }
                (_, Some((i, d))) => {
                    let (_, sound, gain, player) = self.pending.remove(i);
                    self.admit(sound, d, gain, block, player);
                }
                _ => break,
            }
        }
        // 3) 섞기 (들어온 순서대로 더한다)
        let bank = &self.c.chart.bank;
        let bch = bank.channels as usize;
        for v in &self.voices {
            let snd = bank.sounds[v.sound as usize];
            let lo = v.start.max(block);
            let hi = v.end.min(end);
            if hi <= lo {
                continue;
            }
            let src0 = (snd.offset as i64 + (lo - v.start)) as usize * bch;
            let dst0 = (lo - block) as usize * out_ch;
            let frames = (hi - lo) as usize;
            let src = &bank.pcm[src0..src0 + frames * bch];
            let dst = &mut out[dst0..dst0 + frames * out_ch];
            for (d, s) in dst.chunks_exact_mut(out_ch).zip(src.chunks_exact(bch)) {
                d[0] += s[0] * v.gain;
                if out_ch > 1 {
                    d[1] += s[if bch > 1 { 1 } else { 0 }] * v.gain;
                }
            }
        }
        // 4) 끝난(또는 끊긴) 보이스 제거 (순서 유지)
        self.voices.retain(|v| v.end > end && v.end > v.start.max(block));
    }

    /// 모든 이벤트가 끝나고 울리는 소리도 없는가.
    pub fn finished(&self) -> bool {
        (!self.auto_from_chart || self.cursor >= self.c.chart.auto.len()) && self.voices.is_empty() && self.pending.is_empty()
    }
}

/// 오프라인 렌더: 모든 노트를 제때(오차 0) 쳤다고 보고 곡 전체를 스테레오 f32로 만든다 (정답 렌더).
/// `block`: 블록 크기 (결과는 블록 크기와 무관해야 한다 — 시험으로 확인).
pub fn render_perfect(c: &Compiled, block: usize) -> (Vec<f32>, Vec<MixEvent>) {
    let rate = c.chart.bank.rate;
    let total = ns_to_frame(c.chart.end, rate) + 1;
    let mut m = Mixer::new(c, 0, LatePolicy::DEFAULT, 1 << 16);
    // 노트 시각 순으로 키음 요청을 미리 넣는다 (대기열 용량은 노트 소리 수에 맞춰 늘린다: 오프라인이라 허용)
    let mut hits: Vec<(i64, usize, usize)> = Vec::new();
    for (l, lane) in c.chart.lanes.iter().enumerate() {
        for (i, n) in lane.notes.iter().enumerate() {
            hits.push((n.t, l, i));
        }
    }
    hits.sort();
    m.pending = Vec::with_capacity(c.chart.note_sounds.len().max(1));
    for &(t, l, i) in &hits {
        m.play_note(l, i, ns_to_frame(t, rate));
    }
    let mut out = vec![0f32; total as usize * 2];
    let mut log = Vec::new();
    let mut f = 0i64;
    while f < total {
        let n = block.min((total - f) as usize);
        let s = f as usize * 2;
        m.fill(&mut out[s..s + n * 2], 2, f, n);
        m.drain_events(&mut log);
        f += n as i64;
    }
    // 곡 끝 뒤 꼬리 (긴 소리가 end를 넘으면)
    while !m.finished() {
        let n = block;
        let s = out.len();
        out.resize(s + n * 2, 0.0);
        m.fill(&mut out[s..], 2, f, n);
        m.drain_events(&mut log);
        f += n as i64;
    }
    (out, log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{compile, tests::tiny};

    #[test]
    fn block_size_does_not_change_bits() {
        // 자동재생 이벤트를 노트 바로 뒤(1프레임 남짓)로 옮겨, 한 블록 안에서 두 출처가 섞이는 경우도 본다
        let mut ch = tiny();
        ch.auto[1].t = 250_030_000;
        ch.auto[2].t = 250_030_000;
        for c in [compile(tiny()).unwrap(), compile(ch).unwrap()] {
            check_blocks(&c);
        }
    }

    fn check_blocks(c: &Compiled) {
        let (a, la) = render_perfect(c, 1);
        for b in [7, 64, 441, 4096] {
            let (x, lx) = render_perfect(c, b);
            let n = a.len().min(x.len());
            assert_eq!(&a[..n], &x[..n], "블록 {b}");
            assert!(a[n..].iter().chain(x[n..].iter()).all(|&v| v == 0.0));
            let starts = |l: &Vec<MixEvent>| l.iter().filter(|e| matches!(e, MixEvent::Start { .. })).count();
            assert_eq!(starts(&la), starts(&lx));
        }
    }

    #[test]
    fn sounds_land_on_exact_frames_with_gain() {
        let c = compile(tiny()).unwrap();
        let (out, log) = render_perfect(&c, 256);
        // 0.25초 노트: 소리 1(이득 1.0)의 첫 프레임이 정확히 11,025에
        let f = 11_025usize;
        let s1 = c.chart.bank.sounds[1].offset as usize * 2;
        // 그 순간 울리는 다른 소리: 긴 소리 2(0초부터, 이득 1.0)
        let s2 = (c.chart.bank.sounds[2].offset as usize + f) * 2;
        assert_eq!(out[f * 2], c.chart.bank.pcm[s2] + c.chart.bank.pcm[s1]);
        assert!(log.iter().all(|e| matches!(e, MixEvent::Start { late, .. } if *late <= 0)));
        // 시작 사건 수 = 자동 3 + 노트 소리 3
        assert_eq!(log.len(), 6);
    }

    #[test]
    fn now_and_cut_same() {
        let c = compile(tiny()).unwrap();
        let mut m = Mixer::new(&c, 0, LatePolicy::DEFAULT, 64);
        m.set_auto_from_chart(false);
        m.set_cut_same(true);
        let mut out = vec![0f32; 300 * 2];
        m.play_sound(1, 1.0, NOW, false);
        m.fill(&mut out[..100 * 2], 2, 0, 100);
        m.play_sound(1, 1.0, NOW, false); // 같은 소리 다시 → 앞 소리를 100에서 끊음
        m.fill(&mut out[100 * 2..], 2, 100, 200);
        let mut log = Vec::new();
        m.drain_events(&mut log);
        assert!(log.contains(&MixEvent::Cut { sound: 1, at: 100 }));
        assert_eq!(log.iter().filter(|e| matches!(e, MixEvent::Start { late: 0, .. })).count(), 2);
        // 프레임 100의 값 = 두 번째 소리의 첫 샘플만
        let s1 = c.chart.bank.sounds[1].offset as usize * 2;
        assert_eq!(out[100 * 2], c.chart.bank.pcm[s1]);
    }

    #[test]
    fn late_policies() {
        let c = compile(tiny()).unwrap();
        let mut m = Mixer::new(&c, 0, LatePolicy { player: Late::Drop, short: Late::Drop, long: Late::CutFront, drop_after: i64::MAX }, 64);
        let mut out = vec![0f32; 100 * 2];
        // 블록 1,000부터 시작: 긴 소리(0프레임 예정)는 잘라서 원래 자리, 짧은 소리 요청(900)은 버림
        m.play_note(0, 0, 900);
        m.fill(&mut out, 2, 1000, 100);
        let mut log = Vec::new();
        m.drain_events(&mut log);
        assert!(log.contains(&MixEvent::Start { sound: 2, due: 0, start: 0, block: 1000, late: 1000, player: false, gain: 1.0 }));
        assert!(log.contains(&MixEvent::Dropped { sound: 1, due: 900, block: 1000, player: true }));
        let s2 = (c.chart.bank.sounds[2].offset as usize + 1000) * 2;
        assert_eq!(out[0], c.chart.bank.pcm[s2]);
    }
}
