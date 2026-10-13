//! 예약기: 소리를 장치 프레임 위치에 놓는다.
//!
//! 명령 두 가지:
//! - `At { frame }`: 정규 일정의 프레임 위치에서 시작 (샘플 단위). 이미 지났으면 `LatePolicy`를 따른다.
//! - `Now`: 지금 채우는 블록의 맨 앞에서 시작. BMS 구동기들의 "지금 울려"와 같은 방식 (비교용 정책).
//!
//! 프레임 번호 공간: 렌더 스트림에 쓴 프레임 순번. 장치는 같은 번호의 프레임을 순서대로 내보낸다.

use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    At { frame: u64, sound: u32, channel: usize },
    Now { sound: u32, channel: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LatePolicy {
    /// 늦은 만큼 앞부분을 잘라 내고, 남은 부분은 원래 자리에 맞춰 재생 (설계안).
    CutFront,
    /// 늦어도 처음부터 재생 (늦은 만큼 통째로 밀림).
    PlayWhole,
}

/// 소리 하나가 실제로 시작한 기록. 예약 대비 실제 위치를 남긴다.
#[derive(Clone, Copy, Debug)]
pub struct StartLog {
    pub sound: u32,
    pub requested: Option<u64>,
    /// 소리의 0번 샘플이 놓인 프레임 위치 (CutFront면 지난 위치일 수 있음).
    pub start: u64,
    /// 명령을 받아 처리한 블록의 첫 프레임.
    pub block: u64,
}

struct Voice {
    sound: u32,
    start: u64,
    channel: usize,
}

pub struct Mixer {
    pub sounds: Vec<Vec<f32>>,
    pub late_policy: LatePolicy,
    voices: Vec<Voice>,
    pub log: Vec<StartLog>,
}

impl Mixer {
    pub fn new(sounds: Vec<Vec<f32>>, late_policy: LatePolicy) -> Self {
        Mixer { sounds, late_policy, voices: Vec::with_capacity(256), log: Vec::with_capacity(1 << 16) }
    }

    fn accept(&mut self, cmd: Cmd, block: u64) {
        let (sound, channel, requested, start) = match cmd {
            Cmd::At { frame, sound, channel } => {
                let s = if frame < block && self.late_policy == LatePolicy::PlayWhole { block } else { frame };
                (sound, channel, Some(frame), s)
            }
            Cmd::Now { sound, channel } => (sound, channel, None, block),
        };
        self.voices.push(Voice { sound, start, channel });
        self.log.push(StartLog { sound, requested, start, block });
    }

    /// 프레임 [block, block+n) 을 채운다. `out`은 채널 수만큼 끼워 넣은 f32 버퍼 (이미 0으로 채워져 있어야 함).
    pub fn fill(&mut self, out: &mut [f32], channels: usize, block: u64, n: usize, rx: &Receiver<Cmd>) {
        while let Ok(cmd) = rx.try_recv() {
            self.accept(cmd, block);
        }
        let end = block + n as u64;
        for v in &self.voices {
            let snd = &self.sounds[v.sound as usize];
            let (s, e) = (v.start, v.start + snd.len() as u64);
            let lo = s.max(block);
            let hi = e.min(end);
            for f in lo..hi.max(lo) {
                out[(f - block) as usize * channels + v.channel] += snd[(f - s) as usize];
            }
        }
        self.voices.retain(|v| v.start + self.sounds[v.sound as usize].len() as u64 > end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn at_is_sample_exact_and_cut_front_when_late() {
        let (tx, rx) = channel();
        let mut m = Mixer::new(vec![vec![1.0, 2.0, 3.0, 4.0]], LatePolicy::CutFront);
        let mut out = vec![0.0f32; 8 * 2];
        tx.send(Cmd::At { frame: 105, sound: 0, channel: 0 }).unwrap();
        m.fill(&mut out, 2, 100, 8, &rx);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left, vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
        // 다음 블록에서 꼬리 4.0
        let mut out2 = vec![0.0f32; 4 * 2];
        m.fill(&mut out2, 2, 108, 4, &rx);
        assert_eq!(out2[0], 4.0);
        // 늦은 예약: 프레임 110을 블록 112에서 받으면 앞 2샘플을 잘라 3.0부터
        tx.send(Cmd::At { frame: 110, sound: 0, channel: 1 }).unwrap();
        let mut out3 = vec![0.0f32; 4 * 2];
        m.fill(&mut out3, 2, 112, 4, &rx);
        let right: Vec<f32> = out3.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(right, vec![3.0, 4.0, 0.0, 0.0]);
    }
}
