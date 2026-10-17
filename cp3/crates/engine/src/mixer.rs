//! 믹서: 실행 데이터를 출력 스트림 프레임에 섞는다. 오프라인 렌더와 실시간 오디오 콜백이 같은 코드를 쓴다 (CP2 믹서에서 가져와 확장).
//!
//! - 콘텐츠 대응: 곡 시간 0이 놓이는 스트림 프레임 `origin`. 곡 시각 t의 소리는 프레임 `origin + ns_to_frame(t)`에서 시작한다.
//! - 자동재생: `auto`를 커서 하나로 앞에서부터 훑는다. 플레이어 키음은 `play_note`·`play_sound` 요청(목표 프레임)으로 들어온다.
//! - **정규 발음 순서 (명세 3-4)**: 같은 프레임이면 자동재생 먼저, 그다음 요청이 들어온 순서. 같은 소리 끊기·choke도 이 순서로.
//! - **이벤트 시각마다 나눠 섞기 (리뷰 3-0 #6)**: 블록 안에서 다음 발음 시각까지 먼저 섞고, 그 시각에 끝난 보이스를 내보낸 뒤
//!   새 보이스를 올린다. 그래서 보이스 예산 판단·끊기가 블록 크기와 무관하고, 늦은 일이 없으면 결과가 블록 크기와 무관하게 비트 단위로 같다.
//! - 보이스는 고정 크기 풀이고 들어온 순서를 유지한다(더하는 순서 고정).
//! - 소리 형식: f32 또는 16비트 정수(÷ 32768), 이득 = 소리 표 이득 × 이벤트 이득.
//! - 늦은 소리: 긴 소리와 짧은 소리 각각 정책을 따른다 (CP2와 같음).
//!
//! 콜백 안에서 할당·잠금·파일 작업을 하지 않는다 (보이스 풀과 로그 버퍼는 미리 잡는다).

use crate::runtime::{ns_to_frame, Compiled, Pcm2, SoundKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Late {
    /// 늦은 만큼 앞부분을 잘라 내고 남은 부분을 원래 자리에 맞춰 낸다.
    CutFront,
    /// 늦어도 처음부터 낸다 (통째로 밀림).
    PlayWhole,
    /// 내지 않는다.
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatePolicy {
    pub player: Late,
    pub short: Late,
    pub long: Late,
    /// "처음부터(PlayWhole)"인 소리도 이 프레임 수보다 더 늦으면 버린다.
    pub drop_after: i64,
}

impl LatePolicy {
    pub const DEFAULT: LatePolicy = LatePolicy { player: Late::PlayWhole, short: Late::PlayWhole, long: Late::CutFront, drop_after: i64::MAX };
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Voice {
    sound: u32,
    start: i64,
    /// 이 프레임 전은 섞지 않는다 (탐색으로 복원한 보이스: 목표 프레임부터만 들린다, 리뷰 3-3 #7). 보통은 start.
    gate: i64,
    gain: f32,
    end: i64,
    t: i64,
    player: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixEvent {
    Start { sound: u32, due: i64, start: i64, block: i64, late: i64, player: bool, gain: f32, t: i64 },
    Dropped { sound: u32, due: i64, block: i64, player: bool },
    Overflow { sound: u32, due: i64, player: bool },
    /// 같은 소리 다시 울림(cut) 또는 choke로 앞 소리를 프레임 `at`에서 끊었다.
    Cut { sound: u32, at: i64 },
}

/// "지금 울려": 예정 프레임 대신 이 값을 주면 요청을 받은 블록의 맨 앞에서 시작한다.
pub const NOW: i64 = i64::MIN;
/// 곡 시각을 모르는 소리 (빈 누름 등).
pub const NO_T: i64 = i64::MIN;

/// 발음 일정 한 줄: 자동재생, 또는 (자동 연주일 때) PERFECT 자리의 노트 소리.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sched {
    t: i64,
    sound: u32,
    gain: f32,
    player: bool,
}

pub struct Mixer<'a> {
    c: &'a Compiled,
    origin: i64,
    late: LatePolicy,
    /// 정규 발음 순서(명세 3-4)의 일정과 커서
    sched: Vec<Sched>,
    cursor: usize,
    /// 탐색 중: 사건을 기록하지 않는다
    quiet: bool,
    /// 미리보기 버퍼링 (README 4-3 3): 앞으로 이 프레임 수 안의 발음 슬롯이 모두 준비돼야 섞는다 (0 = 끔, 플레이는 끔)
    lookahead: i64,
    /// 준비 확인이 끝난 일정 위치 (준비는 한 번 되면 그대로라 앞으로만 간다)
    ready_cursor: usize,
    /// 버퍼링으로 쉰 횟수·프레임
    pub pauses: u64,
    pub paused_frames: u64,
    voices: Vec<Voice>,
    /// 요청 대기열 (목표 프레임 또는 NOW, 소리, 이득, 플레이어?, 곡 시각)
    pending: Vec<(i64, u32, f32, bool, i64)>,
    auto_from_chart: bool,
    events: Vec<MixEvent>,
    events_cap: usize,
    pub overflows: u64,
    pub events_lost: u64,
    /// 준비 안 된 슬롯을 울린 횟수 (비상: 그 실행은 정답 렌더로 세지 않는다)
    pub unready: u64,
    /// 음량 (3-4 설정): [반주(자동재생), 친 키음]. 사건 기록의 gain은 요청 이득 그대로(정답 렌더 비교가 음량에 흔들리지 않게)
    vol: [f32; 2],
}

impl<'a> Mixer<'a> {
    pub fn new(c: &'a Compiled, origin: i64, late: LatePolicy, events_cap: usize) -> Self {
        Mixer {
            c,
            origin,
            late,
            sched: c.chart.auto.iter().map(|e| Sched { t: e.t, sound: e.sound, gain: e.gain, player: false }).collect(),
            cursor: 0,
            quiet: false,
            lookahead: 0,
            ready_cursor: 0,
            pauses: 0,
            paused_frames: 0,
            voices: Vec::with_capacity(c.voice_budget as usize),
            pending: Vec::with_capacity(512),
            auto_from_chart: true,
            events: Vec::with_capacity(events_cap),
            events_cap,
            overflows: 0,
            events_lost: 0,
            unready: 0,
            vol: [1.0, 1.0],
        }
    }

    /// 반주(자동재생)와 친 키음의 음량. 다음에 올리는 소리부터 적용된다.
    pub fn set_volumes(&mut self, bgm: f32, keys: f32) {
        self.vol = [bgm, keys];
    }

    pub fn set_origin(&mut self, origin: i64) {
        self.origin = origin;
    }
    pub fn origin(&self) -> i64 {
        self.origin
    }
    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }
    pub fn set_auto_from_chart(&mut self, on: bool) {
        self.auto_from_chart = on;
    }
    pub fn auto_cursor(&self) -> usize {
        self.cursor
    }
    /// 자동 연주: 노트 소리도 PERFECT 자리(노트 시각 + offset)에 일정으로 넣는다 (미리보기·정답 렌더).
    /// 순서는 정규 발음 순서: 같은 ns면 자동재생 → 레인 선언 순서 → 노트 순서 → 노트 소리 순서. 제어 경로에서 부른다(할당함).
    pub fn set_autoplay_notes(&mut self) {
        let c = &self.c.chart;
        let mut v: Vec<(i64, u8, usize, usize, usize, Sched)> =
            c.auto.iter().enumerate().map(|(i, e)| (e.t, 0, i, 0, 0, Sched { t: e.t, sound: e.sound, gain: e.gain, player: false })).collect();
        for (l, lane) in c.lanes.iter().enumerate() {
            for (i, n) in lane.notes.iter().enumerate() {
                for k in 0..n.sounds_len as usize {
                    let s = c.note_sounds[n.sounds_start as usize + k];
                    let t = n.t + s.offset;
                    v.push((t, 1, l, i, k, Sched { t, sound: s.sound, gain: s.gain, player: true }));
                }
            }
        }
        v.sort_by_key(|x| (x.0, x.1, x.2, x.3, x.4));
        self.sched = v.into_iter().map(|x| x.5).collect();
        self.cursor = 0;
    }

    /// 미리보기 버퍼링 켜기: 앞으로 `frames` 안의 발음 슬롯이 준비되지 않았으면 그 블록은 섞지 않고 쉰다.
    pub fn set_lookahead(&mut self, frames: i64) {
        self.lookahead = frames;
        self.ready_cursor = self.cursor;
    }

    /// 버퍼링이 필요한가: [block, block_end + lookahead) 동안 울릴 구간을 지금 울리는 보이스와 그 안에서 시작하는 발음의 슬롯이
    /// 모두 덮었나 (전체 슬롯, 또는 그 구간을 덮는 부분 슬롯 — 3-5).
    fn must_pause(&mut self, block: i64, block_end: i64) -> bool {
        if self.lookahead == 0 {
            return false;
        }
        let rate = self.c.chart.bank.rate;
        let bank = &self.c.chart.bank;
        let horizon = block_end + self.lookahead;
        if self.voices.iter().any(|v| !bank.covers(v.sound, block.max(v.gate) - v.start, horizon.min(v.end) - v.start)) {
            return true;
        }
        self.ready_cursor = self.ready_cursor.max(self.cursor);
        while self.ready_cursor < self.sched.len() {
            let e = self.sched[self.ready_cursor];
            let due = self.origin + ns_to_frame(e.t, rate);
            if due >= horizon {
                return false;
            }
            if !bank.covers(e.sound, 0, horizon - due) {
                return true;
            }
            self.ready_cursor += 1;
        }
        false
    }

    /// 쉬기: 콘텐츠 원점과 울리는 보이스·대기 요청을 `n` 프레임 뒤로 (장치 시계는 그대로, 같은 위치에서 재개).
    fn pause(&mut self, n: i64) {
        self.origin += n;
        for v in self.voices.iter_mut() {
            v.start += n;
            v.gate += n;
            v.end += n;
        }
        for p in self.pending.iter_mut().filter(|p| p.0 != NOW) {
            p.0 += n;
        }
        self.pauses += 1;
        self.paused_frames += n as u64;
    }

    /// 탐색 (README 4-3 6): 곡 시각 `t`에서 이어 재생할 상태를 만든다. 목표 프레임 전의 발음을 **같은 `admit` 코드로** 배정만
    /// 다시 돌리고(섞지 않음, 같은 소리 끊기·choke·보이스 예산 포함) 그때 살아 있는 보이스를 그대로 둔다. 보이스는 원래 시작 프레임을
    /// 갖고 있어 중간부터 읽는 위치 = 목표 프레임 − 시작 프레임. 목표 프레임의 발음은 다음 `fill`이 처리한다. 대기 요청은 버린다.
    pub fn seek(&mut self, t: i64) {
        let rate = self.c.chart.bank.rate;
        let target = self.origin + ns_to_frame(t, rate);
        self.voices.clear();
        self.pending.clear();
        self.quiet = true;
        self.cursor = 0;
        while self.cursor < self.sched.len() {
            let e = self.sched[self.cursor];
            let f = self.origin + ns_to_frame(e.t, rate);
            if f >= target {
                break;
            }
            self.cursor += 1;
            self.retire(f);
            self.admit(e.sound, f, e.gain, f, f, e.player, e.t);
        }
        self.retire(target);
        for v in self.voices.iter_mut() {
            v.gate = v.gate.max(target);
        }
        // 준비 안 됨은 실제로 들릴 첫 프레임(탐색 지점) 기준으로 센다 — 배정 중에는 세지 않았다 (부분 슬롯은 탐색 지점부터 덮는다)
        let bank = &self.c.chart.bank;
        self.unready += self.voices.iter().filter(|v| !bank.covers(v.sound, v.gate - v.start, v.gate - v.start + 1)).count() as u64;
        self.quiet = false;
        self.ready_cursor = self.cursor;
    }

    /// 지금 울리는 자동재생 긴 소리(반주)들: (0번 샘플이 놓인 프레임, 그 소리의 곡 시각 ns).
    pub fn long_voices(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.voices
            .iter()
            .filter(|v| !v.player && v.t != NO_T && self.c.chart.bank.sounds[v.sound as usize].kind == SoundKind::Long)
            .map(|v| (v.start, v.t))
    }

    /// 플레이어 키음 요청: 노트의 소리들을 프레임 `frame`(누른 순간의 목표 프레임, 또는 NOW)에서 낸다.
    /// 아르페지오 소리는 `frame + ns_to_frame(offset)`. 명세 3-4의 "목표 ns를 한 번만 프레임으로"와 1프레임까지 다를 수 있다
    /// (누른 순간의 ns를 오디오 스레드가 모름) — 정확히 하려면 `play_note_at`.
    pub fn play_note(&mut self, lane: usize, note: usize, frame: i64) {
        let n = self.c.chart.lanes[lane].notes[note];
        let rate = self.c.chart.bank.rate;
        for s in &self.c.chart.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize] {
            let f = if frame == NOW || s.offset == 0 { frame } else { frame + ns_to_frame(s.offset, rate) };
            self.push_req(f, s.sound, s.gain, true, n.t + s.offset);
        }
    }

    /// 노트 소리를 (누른 시각의 청취 곡 ns + D) 기준으로 예약: 소리마다 `frame_of(press_ns + offset)` 하나로 바꾼다 (명세 3-4).
    pub fn play_note_at(&mut self, lane: usize, note: usize, press_frame_of: impl Fn(i64) -> i64, press_ns: i64) {
        let n = self.c.chart.lanes[lane].notes[note];
        for s in &self.c.chart.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize] {
            self.push_req(press_frame_of(press_ns + s.offset), s.sound, s.gain, true, n.t + s.offset);
        }
    }

    pub fn play_sound(&mut self, sound: u32, gain: f32, frame: i64, player: bool) {
        self.push_req(frame, sound, gain, player, NO_T);
    }

    pub fn play_sound_t(&mut self, sound: u32, gain: f32, frame: i64, player: bool, t: i64) {
        self.push_req(frame, sound, gain, player, t);
    }

    fn push_req(&mut self, frame: i64, sound: u32, gain: f32, player: bool, t: i64) {
        if self.pending.len() < self.pending.capacity() {
            self.pending.push((frame, sound, gain, player, t));
        } else {
            self.overflows += 1;
        }
    }

    pub fn drain_events(&mut self, into: &mut Vec<MixEvent>) {
        into.extend(self.events.drain(..));
    }

    pub fn drain_each(&mut self, mut f: impl FnMut(MixEvent)) {
        for e in self.events.drain(..) {
            f(e);
        }
    }

    fn log(&mut self, e: MixEvent) {
        if self.quiet {
            return;
        }
        if self.events.len() < self.events_cap {
            self.events.push(e);
        } else {
            self.events_lost += 1;
        }
    }

    /// 프레임 `at`에 끝난 보이스를 내보낸다 (순서 유지).
    fn retire(&mut self, at: i64) {
        self.voices.retain(|v| v.end > at);
    }

    /// 새 소리를 올린다. `at` = 지금 섞고 있는 위치(블록 안), `block` = 블록 시작(늦음 판단).
    fn admit(&mut self, sound: u32, due: i64, gain: f32, at: i64, block: i64, player: bool, t: i64) {
        let snd = self.c.chart.bank.sounds[sound as usize];
        let frames = snd.frames as i64;
        let late = block - due;
        let start = if late <= 0 {
            due
        } else {
            let p = match (player, snd.kind) {
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
        if start + frames <= at {
            self.log(MixEvent::Dropped { sound, due, block, player });
            return;
        }
        let from = start.max(at);
        // 같은 소리 다시 울림(cut)·choke: 지금 울리는 앞 소리를 `from`에서 끊는다
        let choke = snd.choke;
        let mut cut_sounds = [0u32; 8];
        let mut ncut = 0usize;
        for v in self.voices.iter_mut().filter(|v| v.end > from) {
            let other = self.c.chart.bank.sounds[v.sound as usize];
            if (v.sound == sound && snd.cut) || (choke != 0 && other.choke == choke && v.sound != sound) {
                v.end = from;
                if ncut < cut_sounds.len() {
                    cut_sounds[ncut] = v.sound;
                    ncut += 1;
                }
            }
        }
        for &s in &cut_sounds[..ncut] {
            self.log(MixEvent::Cut { sound: s, at: from });
        }
        // 예산: 지금(`from`) 울리는 보이스 수 (끊긴·끝난 것은 세지 않음)
        // 끊긴 보이스는 그 자리(`from` = 지금 섞은 위치)에서 바로 회수한다 (리뷰 3-3 #2): 실제로 울리는 보이스만 예산·용량을 차지하고,
        // seek(이벤트마다 회수)와 fill(같은 프레임 이벤트 사이)의 상태 전이가 같아진다.
        self.retire(from);
        if self.voices.len() >= self.c.voice_budget as usize || self.voices.len() >= self.voices.capacity() {
            self.overflows += 1;
            self.log(MixEvent::Overflow { sound, due, player });
            return;
        }
        if !self.quiet && !self.c.chart.bank.covers(sound, from - start, from - start + 1) {
            self.unready += 1;
        }
        self.voices.push(Voice { sound, start, gate: start, gain: gain * snd.gain * self.vol[player as usize], end: start + frames, t, player });
        self.log(MixEvent::Start { sound, due, start, block, late, player, gain, t });
    }

    /// 스트림 프레임 [from, to)을 섞는다 (out의 0번 = 프레임 block).
    fn mix_range(&self, out: &mut [f32], out_ch: usize, block: i64, from: i64, to: i64) {
        if to <= from {
            return;
        }
        let bank = &self.c.chart.bank;
        for v in &self.voices {
            let lo = v.start.max(v.gate).max(from);
            let hi = v.end.min(to);
            if hi <= lo {
                continue;
            }
            let snd = bank.sounds[v.sound as usize];
            // 준비 안 된 슬롯(비상, README 4-3 3): 보이스 상태는 그대로 두고 출력만 비운다 (admit에서 셌다).
            // 전체 슬롯이 없으면 이 구간을 덮는 부분 슬롯(3-5)을 읽는다 — 같은 구간은 비트까지 같다.
            let Some((data, pfrom, pend)) = bank.view(snd.data as usize, lo - v.start) else { continue };
            let hi = hi.min(v.start + pend); // 슬롯이 예상보다 짧으면(디코딩 실패 → 빈 슬롯) 또는 부분 슬롯 끝까지만
            if hi <= lo {
                continue;
            }
            let src0 = (lo - v.start - pfrom) as usize * 2;
            let dst0 = (lo - block) as usize * out_ch;
            let frames = (hi - lo) as usize;
            let dst = &mut out[dst0..dst0 + frames * out_ch];
            match data {
                Pcm2::F32(x) => {
                    let src = &x[src0..src0 + frames * 2];
                    for (d, s) in dst.chunks_exact_mut(out_ch).zip(src.chunks_exact(2)) {
                        d[0] += s[0] * v.gain;
                        if out_ch > 1 {
                            d[1] += s[1] * v.gain;
                        }
                    }
                }
                Pcm2::I16(x) => {
                    let src = &x[src0..src0 + frames * 2];
                    let g = v.gain / 32768.0;
                    for (d, s) in dst.chunks_exact_mut(out_ch).zip(src.chunks_exact(2)) {
                        d[0] += s[0] as f32 * g;
                        if out_ch > 1 {
                            d[1] += s[1] as f32 * g;
                        }
                    }
                }
            }
        }
    }

    /// 스트림 프레임 [block, block + n)을 채운다. `out`은 출력 채널 수(`out_ch`)만큼 끼워 넣은 버퍼이고 0으로 초기화돼 있어야 한다.
    pub fn fill(&mut self, out: &mut [f32], out_ch: usize, block: i64, n: usize) {
        let end = block + n as i64;
        if self.must_pause(block, end) {
            self.pause(n as i64);
            return;
        }
        let rate = self.c.chart.bank.rate;
        for p in self.pending.iter_mut() {
            if p.0 == crate::mixer::NOW {
                p.0 = block;
            }
        }
        let mut pos = block;
        loop {
            // 다음 발음: 자동재생(같은 프레임이면 먼저) 또는 요청 (가장 이른 예정 프레임, 같으면 들어온 순서)
            let auto_due = (self.auto_from_chart && self.cursor < self.sched.len())
                .then(|| self.origin + ns_to_frame(self.sched[self.cursor].t, rate))
                .filter(|&d| d < end);
            let mut best: Option<(usize, i64)> = None;
            for (i, &(d, _, _, _, _)) in self.pending.iter().enumerate() {
                if d < end && best.map_or(true, |(_, b)| d < b) {
                    best = Some((i, d));
                }
            }
            let (due, is_auto) = match (auto_due, best) {
                (Some(a), b) if b.map_or(true, |(_, p)| a <= p) => (a, true),
                (_, Some((_, d))) => (d, false),
                _ => break,
            };
            // 그 시각까지 먼저 섞고, 그 시각에 끝난 보이스를 내보낸다
            let at = due.max(block);
            if at > pos {
                self.mix_range(out, out_ch, block, pos, at);
                pos = at;
                self.retire(pos);
            }
            if is_auto {
                let e = self.sched[self.cursor];
                self.cursor += 1;
                self.admit(e.sound, due, e.gain, pos, block, e.player, e.t);
            } else {
                let (i, _) = best.unwrap();
                let (_, sound, gain, player, t) = self.pending.remove(i);
                self.admit(sound, due, gain, pos, block, player, t);
            }
        }
        self.mix_range(out, out_ch, block, pos, end);
        self.retire(end);
    }

    pub fn finished(&self) -> bool {
        (!self.auto_from_chart || self.cursor >= self.sched.len()) && self.voices.is_empty() && self.pending.is_empty()
    }
}

/// 오프라인 렌더: 모든 노트를 제때(오차 0) 쳤다고 보고 곡 전체를 스테레오 f32로 만든다 (정답 렌더).
/// 노트 소리는 정규 발음 순서(명세 3-4: 같은 ns면 자동재생 → 레인 선언 순서 → 노트 소리 순서)로 요청한다.
pub fn render_perfect(c: &Compiled, block: usize) -> (Vec<f32>, Vec<MixEvent>) {
    render_from(c, block, 0).map_or_else(|| (Vec::new(), Vec::new()), |x| x)
}

/// 곡 시각 `from`부터의 정답 렌더 (`from` > 0이면 탐색한 뒤). 출력 0번 프레임 = 곡 시각 `from`의 프레임.
pub fn render_from(c: &Compiled, block: usize, from: i64) -> Option<(Vec<f32>, Vec<MixEvent>)> {
    let rate = c.chart.bank.rate;
    let total = ns_to_frame(c.chart.end, rate) + 1;
    let mut m = Mixer::new(c, 0, LatePolicy::DEFAULT, 1 << 18);
    m.set_autoplay_notes();
    let f0 = ns_to_frame(from, rate);
    if from > 0 {
        m.seek(from);
    }
    if f0 >= total {
        return None;
    }
    let mut out = vec![0f32; (total - f0) as usize * 2];
    let mut log = Vec::new();
    let mut f = f0;
    while f < total {
        let n = block.min((total - f) as usize);
        let s = (f - f0) as usize * 2;
        m.fill(&mut out[s..s + n * 2], 2, f, n);
        m.drain_events(&mut log);
        f += n as i64;
    }
    while !m.finished() {
        let n = block;
        let s = out.len();
        out.resize(s + n * 2, 0.0);
        m.fill(&mut out[s..], 2, f, n);
        m.drain_events(&mut log);
        f += n as i64;
    }
    Some((out, log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::*;

    /// 시험용 작은 곡: 44.1 kHz, 짧은 소리 2개 + 긴 소리 1개, 자동재생 3개, 레인 1개에 노트 2개.
    pub fn tiny(budget: u32, cut1: bool) -> Compiled {
        let rate = 44_100;
        let (a, b, c) = (100u32, 200u32, 3 * rate);
        let total = (a + b + c) as usize;
        let pcm: Vec<f32> = (0..total * 2).map(|i| (i % 7) as f32 * 0.01 + 0.001).collect();
        let slot = |lo: u32, n: u32| {
            let l = std::sync::OnceLock::new();
            let _ = l.set(Pcm2::F32(pcm[lo as usize * 2..(lo + n) as usize * 2].to_vec().into_boxed_slice()));
            std::sync::Arc::new(l)
        };
        let data = vec![slot(0, a), slot(a, b), slot(a + b, c)];
        let snd = |d: u32, frames: u32, kind| SoundRef { data: d, frames, kind, gain: 1.0, cut: false, choke: 0 };
        let mut s1 = snd(1, b, SoundKind::Short);
        s1.cut = cut1;
        Compiled {
            chart: Chart {
                title: "tiny".into(),
                chart_id: "t".into(),
                layout: "1".into(),
                bank: SoundBank { rate, data, sounds: vec![snd(0, a, SoundKind::Short), s1, snd(2, c, SoundKind::Long)], partial: vec![] },
                auto: vec![
                    SoundEvent { t: 0, sound: 2, gain: 1.0 },
                    SoundEvent { t: 500_000_000, sound: 0, gain: 0.5 },
                    SoundEvent { t: 500_000_000, sound: 0, gain: 0.5 },
                ],
                note_sounds: vec![
                    NoteSound { offset: 0, sound: 1, gain: 1.0 },
                    NoteSound { offset: 0, sound: 0, gain: 0.8 },
                    NoteSound { offset: 1_000_000, sound: 1, gain: 0.8 },
                ],
                lanes: vec![Lane {
                    id: "K1".into(),
                    role: Role::Key,
                    notes: vec![
                        Note { t: 250_000_000, end: 250_000_000, sounds_start: 0, sounds_len: 1, kind: NoteKind::Tap, damage: -1 },
                        Note { t: 750_000_000, end: 1_000_000_000, sounds_start: 1, sounds_len: 2, kind: NoteKind::Long, damage: -1 },
                    ],
                    lane_sounds: vec![],
                }],
                groups: vec![],
                scroll: vec![ScrollPoint { t: 0, pos: 0.0, speed: 2.0 }],
                bars: vec![],
                reference_scroll: 120.0,
                long_rule: LongRule::Cn,
                empty_lane: EmptyLane::PreviousNote,
                end: 3_000_000_000,
                chart_hash: [0; 32],
                variant_choice: vec![],
            },
            voice_budget: budget,
        }
    }

    fn check_blocks(c: &Compiled) {
        let (a, la) = render_perfect(c, 1);
        for b in [7, 64, 441, 4096] {
            let (x, lx) = render_perfect(c, b);
            let n = a.len().min(x.len());
            assert_eq!(&a[..n], &x[..n], "블록 {b}");
            assert!(a[n..].iter().chain(x[n..].iter()).all(|&v| v == 0.0));
            // 블록에 따라 달라지는 칸(block·late)을 뺀 사건: 무엇이 언제 어디서 시작·끊겼는가
            let kinds = |l: &Vec<MixEvent>| {
                let mut v: Vec<String> = l
                    .iter()
                    .map(|e| match *e {
                        MixEvent::Start { sound, due, start, player, t, .. } => format!("start {sound} {due} {start} {player} {t}"),
                        MixEvent::Dropped { sound, due, player, .. } => format!("drop {sound} {due} {player}"),
                        e => format!("{e:?}"),
                    })
                    .collect();
                v.sort();
                v
            };
            assert_eq!(la.len(), lx.len(), "블록 {b}: 사건 수");
            assert_eq!(kinds(&la), kinds(&lx), "블록 {b}: 사건 종류");
        }
    }

    #[test]
    fn block_size_does_not_change_bits() {
        check_blocks(&tiny(16, false));
        check_blocks(&tiny(16, true));
    }

    #[test]
    fn tight_budget_is_block_independent() {
        // 리뷰 3-0 #6 반례: 예산 1, 앞 소리가 끝난 직후 다음 소리 — 블록이 커도 앞 보이스가 자리를 비운 뒤 받아들여야 한다
        for budget in [1, 2] {
            check_blocks(&tiny(budget, false));
            check_blocks(&tiny(budget, true));
        }
    }

    #[test]
    fn cut_same_and_offset() {
        let c = tiny(16, true);
        let (_out, log) = render_perfect(&c, 256);
        // 소리 1을 0.75초 + 1 ms에 다시 울리는 아르페지오는 0.25초의 소리 1(200프레임 = 4.5 ms)과 겹치지 않으므로 끊기 없음
        assert!(!log.iter().any(|e| matches!(e, MixEvent::Cut { .. })));
        // 아르페지오 소리는 0.751초 프레임에
        let f = ns_to_frame(751_000_000, 44_100);
        assert!(log.iter().any(|e| matches!(e, MixEvent::Start { sound: 1, start, .. } if *start == f)));
    }

    #[test]
    fn seek_render_matches_full_render() {
        // README 4-3 완료 조건 1: 처음부터 렌더해 자른 구간 = 탐색 뒤 렌더. 작은 예산·같은 소리 연타(cut)·같은 시각 이벤트·긴 꼬리 포함
        for (budget, cut) in [(16, false), (16, true), (1, false), (2, true)] {
            let c = tiny(budget, cut);
            let (full, _) = render_perfect(&c, 333);
            for t in [1, 250_000_000, 499_999_999, 500_000_000, 500_022_676, 760_000_000, 1_234_567_890, 2_999_000_000] {
                let f0 = ns_to_frame(t, 44_100) as usize;
                let (part, _) = render_from(&c, 97, t).unwrap();
                let n = part.len().min(full.len() - f0 * 2);
                assert_eq!(&part[..n], &full[f0 * 2..f0 * 2 + n], "예산 {budget} cut {cut} 탐색 {t}");
            }
        }
    }

    #[test]
    fn cut_frees_slot_under_tight_budget() {
        // 리뷰 3-3 #2 반례: 예산 1, 긴 소리 A가 울리는 중 같은 프레임에 A 재발음(cut) → 이어 B. 끊긴 A의 슬롯이 회수돼 새 A가 들어가고,
        // B는 예산 초과로 거부. seek로 같은 지점을 지나도 같은 상태여야 한다.
        let mut c = tiny(1, false);
        c.chart.bank.sounds[2].cut = true;
        c.chart.auto = vec![
            SoundEvent { t: 0, sound: 2, gain: 1.0 },
            SoundEvent { t: 500_000_000, sound: 2, gain: 1.0 },
            SoundEvent { t: 500_000_000, sound: 0, gain: 1.0 },
        ];
        c.chart.lanes[0].notes.clear();
        let (full, log) = render_perfect(&c, 64);
        let starts = log.iter().filter(|e| matches!(e, MixEvent::Start { .. })).count();
        let over = log.iter().filter(|e| matches!(e, MixEvent::Overflow { .. })).count();
        assert_eq!((starts, over), (2, 1));
        for t in [400_000_000, 500_000_000, 600_000_000] {
            let f0 = ns_to_frame(t, 44_100) as usize;
            let (part, _) = render_from(&c, 50, t).unwrap();
            let n = part.len().min(full.len() - f0 * 2);
            assert_eq!(&part[..n], &full[f0 * 2..f0 * 2 + n], "탐색 {t}");
        }
    }

    #[test]
    fn seek_gate_hides_earlier_frames() {
        // 리뷰 3-3 #7: 탐색 목표 프레임 전에는 복원한 보이스가 들리지 않는다 (장치 리드인 동안 무음)
        let c = tiny(16, false);
        let mut m = Mixer::new(&c, 1000, LatePolicy::DEFAULT, 64);
        m.set_autoplay_notes();
        m.seek(1_000_000_000); // 목표 프레임 = 1000 + 44100, 긴 소리(곡 0초)가 울리는 중
        let mut out = vec![0f32; 2 * 2000];
        m.fill(&mut out, 2, 44_100 + 1000 - 1000, 2000);
        assert!(out[..2000].iter().all(|&x| x == 0.0)); // 목표 전 1000프레임은 무음
        assert!(out[2000..].iter().any(|&x| x != 0.0)); // 목표부터 소리
    }

    #[test]
    fn buffering_resumes_at_same_place() {
        // README 4-3 완료 조건 2: 준비를 늦추면 누락 없이 쉬었다가 같은 위치에서 재개 → 출력 = 쉰 만큼 0 + 정답 렌더
        let full = render_perfect(&tiny(16, true), 256).0;
        let mut c = tiny(16, true);
        let late = c.chart.bank.data[2].get().unwrap().clone();
        c.chart.bank.data[2] = std::sync::Arc::new(std::sync::OnceLock::new()); // 긴 소리(곡 시각 0) 슬롯을 비운다
        let mut m = Mixer::new(&c, 0, LatePolicy::DEFAULT, 1 << 12);
        m.set_autoplay_notes();
        m.set_lookahead(4410);
        let (blk, paused_blocks) = (256usize, 5);
        let mut out = Vec::new();
        let mut f = 0i64;
        for k in 0.. {
            if k == paused_blocks {
                let _ = c.chart.bank.data[2].set(late.clone());
            }
            let mut b = vec![0f32; blk * 2];
            m.fill(&mut b, 2, f, blk);
            out.extend_from_slice(&b);
            f += blk as i64;
            if k > paused_blocks && m.finished() {
                break;
            }
        }
        assert_eq!(m.pauses, paused_blocks as u64);
        assert_eq!(m.unready, 0);
        let skip = paused_blocks * blk * 2;
        assert!(out[..skip].iter().all(|&x| x == 0.0));
        let n = full.len().min(out.len() - skip);
        assert_eq!(&out[skip..skip + n], &full[..n]);
    }

    #[test]
    fn buffering_after_seek_and_mid_play() {
        // 리뷰 3-3 검증 맹점: 탐색 뒤 첫 준비 부족, 재생 도중(0.5초 소리) 준비 부족 → 출력 = 쉰 블록만큼 0이 끼어든 탐색 렌더
        let blk = 128usize;
        let t0 = 200_000_000;
        let full = render_from(&tiny(16, true), blk, t0).unwrap().0;
        let mut c = tiny(16, true);
        let s0 = c.chart.bank.data[0].get().unwrap().clone(); // 0.5초 자동재생 소리
        c.chart.bank.data[0] = std::sync::Arc::new(std::sync::OnceLock::new());
        let mut m = Mixer::new(&c, 0, LatePolicy::DEFAULT, 1 << 12);
        m.set_autoplay_notes();
        m.set_lookahead(2205); // 50 ms
        m.seek(t0);
        let f0 = ns_to_frame(t0, 44_100);
        let mut out = Vec::new();
        let mut f = f0;
        let mut paused_at = Vec::new();
        for k in 0..400 {
            if k == 200 {
                let _ = c.chart.bank.data[0].set(s0.clone());
            }
            let before = m.pauses;
            let mut b = vec![0f32; blk * 2];
            m.fill(&mut b, 2, f, blk);
            if m.pauses > before {
                paused_at.push(out.len() / 2);
            }
            out.extend_from_slice(&b);
            f += blk as i64;
        }
        assert!(!paused_at.is_empty());
        assert_eq!(m.unready, 0);
        // 쉰 블록을 빼면 탐색 렌더와 같다
        let mut kept = Vec::new();
        for (i, ch) in out.chunks(blk * 2).enumerate() {
            if !paused_at.contains(&(i * blk)) {
                kept.extend_from_slice(ch);
            }
        }
        let n = kept.len().min(full.len());
        assert_eq!(&kept[..n], &full[..n]);
    }

    #[test]
    fn i16_sounds_scale() {
        let mut c = tiny(16, false);
        let l = std::sync::OnceLock::new();
        let _ = l.set(Pcm2::I16(vec![16384i16; 2 * 100].into_boxed_slice()));
        c.chart.bank.data.push(std::sync::Arc::new(l));
        c.chart.bank.sounds[0] = SoundRef { data: 3, frames: 100, kind: SoundKind::Short, gain: 1.0, cut: false, choke: 0 };
        c.chart.auto = vec![SoundEvent { t: 0, sound: 0, gain: 1.0 }];
        c.chart.lanes[0].notes.clear();
        let (out, _) = render_perfect(&c, 64);
        assert_eq!(out[0], 0.5);
    }

    #[test]
    fn volumes_scale_bgm_and_keys_separately() {
        let c = tiny(16, false);
        let run = |bgm: f32, keys: f32| {
            let mut m = Mixer::new(&c, 0, LatePolicy::DEFAULT, 1024);
            m.set_volumes(bgm, keys);
            m.play_note(0, 0, 0);
            let mut out = vec![0f32; 2 * 4096];
            m.fill(&mut out, 2, 0, 4096);
            out
        };
        let base = run(1.0, 1.0);
        let only_bgm = run(1.0, 0.0);
        let only_keys = run(0.0, 1.0);
        let half = run(0.5, 0.5);
        for i in 0..base.len() {
            assert!((only_bgm[i] + only_keys[i] - base[i]).abs() < 1e-6);
            assert!((half[i] - base[i] * 0.5).abs() < 1e-6);
        }
        assert!(only_keys.iter().any(|x| *x != 0.0) && only_bgm.iter().any(|x| *x != 0.0));
    }

    #[test]
    fn scroll_base_choices() {
        // 노트 0 ~ 10초. 속도(박/초): 0~1초 2, 1~1.0001초 순간 이동 9999, 그 뒤 ~7초 3, 7~10초 1.5 (+ 4~4.2초 정지)
        let mut c = tiny(16, false);
        let ch = &mut c.chart;
        ch.lanes[0].notes = vec![
            Note { t: 0, end: 0, sounds_start: 0, sounds_len: 0, kind: NoteKind::Tap, damage: -1 },
            Note { t: 10_000_000_000, end: 10_000_000_000, sounds_start: 0, sounds_len: 0, kind: NoteKind::Tap, damage: -1 },
        ];
        ch.scroll = vec![
            ScrollPoint { t: 0, pos: 0.0, speed: 2.0 },
            ScrollPoint { t: 1_000_000_000, pos: 2.0, speed: 9999.0 },
            ScrollPoint { t: 1_000_100_000, pos: 3.0, speed: 3.0 },
            ScrollPoint { t: 4_000_000_000, pos: 12.0, speed: 0.0 },
            ScrollPoint { t: 4_200_000_000, pos: 12.0, speed: 3.0 },
            ScrollPoint { t: 7_000_000_000, pos: 20.4, speed: 1.5 },
        ];
        assert_eq!(ch.scroll_base("chart"), 120.0);
        assert_eq!(ch.scroll_base("start"), 120.0);
        assert!((ch.scroll_base("mode") - 180.0).abs() < 1e-9);
        assert!((ch.scroll_base("max") - 180.0).abs() < 1e-9); // 순간 이동(0.1 ms)은 뺀다
        assert!((ch.scroll_base("min") - 90.0).abs() < 1e-9);
        assert_eq!(ch.scroll_base("모름"), 120.0);
    }

    #[test]
    fn partial_slot_mixes_like_full() {
        // 긴 소리(슬롯 2)를 탐색 지점 근처만 부분 슬롯으로 두고 섞은 것 = 전체 슬롯으로 섞은 것 (그 구간)
        let full = tiny(16, false);
        let mut part = tiny(16, false);
        let pcm = full.chart.bank.data[2].get().unwrap().clone();
        let Pcm2::F32(x) = &pcm else { panic!() };
        let (from, to) = (40_000i64, 90_000i64);
        let lock = std::sync::OnceLock::new();
        let _ = lock.set(crate::runtime::Partial { from, pcm: Pcm2::F32(x[from as usize * 2..to as usize * 2].to_vec().into_boxed_slice()) });
        part.chart.bank.partial = vec![Default::default(), Default::default(), std::sync::Arc::new(lock)];
        part.chart.bank.data[2] = std::sync::Arc::new(std::sync::OnceLock::new());
        let t = (from as f64 / 44_100.0 * 1e9) as i64 + 10_000_000;
        let run = |c: &Compiled| {
            let mut m = Mixer::new(c, 0, LatePolicy::DEFAULT, 1024);
            m.set_autoplay_notes();
            m.seek(t);
            m.set_lookahead(4096);
            let mut out = vec![0f32; 2 * 8192];
            m.fill(&mut out, 2, ns_to_frame(t, 44_100), 8192);
            (out, m.pauses, m.unready)
        };
        let (a, pa, ua) = run(&full);
        let (b, pb, ub) = run(&part);
        assert_eq!((pa, ua), (0, 0));
        assert_eq!((pb, ub), (0, 0), "부분 슬롯이 덮으면 쉬지 않는다");
        assert_eq!(a, b);
        // 부분 슬롯 끝을 넘는 곳까지 앞을 봐야 하면 쉰다 (건너뛰지 않음)
        let mut m = Mixer::new(&part, 0, LatePolicy::DEFAULT, 1024);
        m.set_autoplay_notes();
        let t2 = (85_000.0 / 44_100.0 * 1e9) as i64;
        m.seek(t2);
        m.set_lookahead(44_100);
        let mut out = vec![0f32; 2 * 1024];
        m.fill(&mut out, 2, ns_to_frame(t2, 44_100), 1024);
        assert_eq!(m.pauses, 1);
    }
}