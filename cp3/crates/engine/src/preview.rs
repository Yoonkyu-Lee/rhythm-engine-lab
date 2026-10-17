//! 미리보기 소리 저장소 (README 4-3 2~5): 차분 하나의 계획(`load::Plan`)을 들을 곳부터 채우는 작업 스레드들.
//!
//! - 준비 순서: 시작 지점에 이미 울리고 있어야 하는 소리와 앞으로 W 안에 시작하는 소리(= 준비 집합)를 먼저, 나머지는
//!   **지금 위치 이후 다음에 쓰이는 시각** 순. 탐색하면 남은 작업 순서를 새 위치로 다시 정한다(`reorder`).
//! - 슬롯은 다 만든 뒤 한 번만 게시한다(`OnceLock`). 믹서는 준비된 슬롯만 읽고, 앞으로 W 안이 준비되지 않으면 쉰다(버퍼링).
//! - 취소: 세대 번호. 작업 스레드는 슬롯 하나를 시작하기 전과 디코딩·리샘플링 사이에 세대를 확인하고, 다르면 게시하지 않고 멈춘다.
//!   곡을 바꾸면 이 저장소를 버린다(곡 사이 캐시 없음, 4-3 5) — 해제는 믹서가 더는 읽지 않을 때(Arc가 마지막으로 놓일 때).
//! - 같은 소리 데이터를 쓰는 슬롯이 여럿이면(BMSON 조각 등) 디코딩 결과를 그 슬롯들이 끝날 때까지 공유한다.

use crate::load::{decode_slot, Plan};
use crate::resample::Filters;
use crate::runtime::{ns_to_frame, Compiled, Pcm2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use track::audio::{self, Pcm};

/// 슬롯별 (시작 지점에 울리고 있음, 시작 지점 이후 다음에 쓰이는 시각 ns)
fn slot_use(c: &Compiled, t0: i64) -> Vec<(bool, i64)> {
    let bank = &c.chart.bank;
    let mut u = vec![(false, i64::MAX); bank.data.len()];
    let mut mark = |sound: u32, t: i64| {
        let s = bank.sounds[sound as usize];
        let e = &mut u[s.data as usize];
        if t <= t0 && t + bank.duration_ns(sound) > t0 {
            e.0 = true;
        }
        if t >= t0 && t < e.1 {
            e.1 = t;
        }
    };
    for a in &c.chart.auto {
        mark(a.sound, a.t);
    }
    for l in &c.chart.lanes {
        for n in &l.notes {
            for s in &c.chart.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize] {
                mark(s.sound, n.t + s.offset);
            }
        }
    }
    u
}

/// 준비 순서 (슬롯 번호): 시작 지점에 울리는 것 → 다음 사용 시각 순 → 시작 지점 뒤로 쓰이지 않는 것.
fn order(c: &Compiled, jobs: &[u32], t0: i64) -> Vec<u32> {
    let u = slot_use(c, t0);
    let mut v = jobs.to_vec();
    v.sort_by_key(|&s| {
        let (on, next) = u[s as usize];
        (!on, next, s)
    });
    v
}

pub struct Store {
    pub plan: Arc<Plan>,
    /// 남은 작업 (슬롯 번호), 앞에서부터 꺼낸다
    queue: Arc<Mutex<Vec<u32>>>,
    /// 이 저장소의 세대 (요청 당시 값). 전역 세대(`current`)와 다르면 작업을 멈춘다.
    gen: u64,
    current: Arc<AtomicU64>,
    workers: Vec<JoinHandle<()>>,
    pub done: Arc<AtomicUsize>,
    pub failed: Arc<AtomicUsize>,
    /// 실패한 슬롯과 이유 (빈 소리로 게시했다 — 영원히 "준비 중"으로 남지 않게, 리뷰 3-3 #8)
    pub errors: Arc<Mutex<Vec<(u32, String)>>>,
}

/// 같은 소리 데이터를 쓰는 슬롯 여럿의 디코딩을 한 번으로 (리뷰 3-3 #10): 칸 하나를 먼저 잡은 작업자가 디코딩하고, 나머지는 기다렸다 받는다.
type Shared = Arc<Mutex<HashMap<[u8; 32], (Arc<OnceLock<Option<Arc<Pcm>>>>, usize)>>>;

impl Store {
    /// 작업 스레드 `threads`개로 채우기 시작. `t0` = 듣기 시작할 곡 시각, `gen` = 요청 당시 세대 (리뷰 3-3 #9: 계획하는 동안
    /// 취소됐으면 작업자를 띄우지 않는다 — 최신 세대를 읽어 낡은 작업이 새 세대로 둔갑하지 않게).
    pub fn start(plan: Plan, t0: i64, threads: usize, current: Arc<AtomicU64>, gen: u64) -> Store {
        let jobs: Vec<u32> = plan.jobs.iter().map(|j| j.slot).collect();
        let q = order(&plan.compiled, &jobs, t0);
        let plan = Arc::new(plan);
        let queue = Arc::new(Mutex::new(q));
        let done = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let filters = Arc::new(Filters::default());
        let shared: Shared = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut sh = shared.lock().unwrap();
            for j in &plan.jobs {
                sh.entry(j.hash).or_insert_with(|| (Arc::new(OnceLock::new()), 0)).1 += 1;
            }
        }
        let live = current.load(Ordering::Acquire) == gen;
        let workers = if !live {
            Vec::new()
        } else {
            (0..threads.max(1))
                .map(|_| {
                    let (plan, queue, current, done, failed, errors, filters, shared) =
                        (plan.clone(), queue.clone(), current.clone(), done.clone(), failed.clone(), errors.clone(), filters.clone(), shared.clone());
                    std::thread::Builder::new()
                        .name("preview-decode".into())
                        .spawn(move || loop {
                            if current.load(Ordering::Acquire) != gen {
                                return;
                            }
                            let Some(pos) = ({
                                let mut q = queue.lock().unwrap();
                                (!q.is_empty()).then(|| q.remove(0))
                            }) else {
                                return;
                            };
                            let j = *plan.jobs.iter().find(|j| j.slot == pos).unwrap();
                            let p = &plan.datas[&j.hash];
                            let cell = shared.lock().unwrap()[&j.hash].0.clone();
                            let pcm = cell.get_or_init(|| audio::decode(p).ok().map(Arc::new)).clone();
                            let slot = &plan.compiled.chart.bank.data[j.slot as usize];
                            let r = match pcm {
                                None => Err("디코딩 실패".to_string()),
                                Some(pcm) => {
                                    if current.load(Ordering::Acquire) != gen {
                                        return; // 취소: 디코딩과 자르기·리샘플링 사이
                                    }
                                    decode_slot(p, &pcm, j.start, j.length, plan.compiled.chart.bank.rate, &filters)
                                }
                            };
                            match r {
                                Ok(x) => {
                                    if current.load(Ordering::Acquire) == gen {
                                        let _ = slot.set(x);
                                        done.fetch_add(1, Ordering::Release);
                                    }
                                }
                                Err(e) => {
                                    // 빈 소리로 게시: 믹서는 무음으로 지나가고(슬롯 길이까지만 읽음), 화면은 실패를 보여 준다
                                    let _ = slot.set(Pcm2::F32(Box::new([])));
                                    failed.fetch_add(1, Ordering::Release);
                                    errors.lock().unwrap().push((j.slot, e));
                                }
                            }
                            let mut sh = shared.lock().unwrap();
                            if let Some(e) = sh.get_mut(&j.hash) {
                                e.1 -= 1;
                                if e.1 == 0 {
                                    sh.remove(&j.hash); // 이 데이터를 쓰는 슬롯이 다 끝났다 → 디코딩 결과 해제
                                }
                            }
                        })
                        .expect("미리보기 디코딩 스레드")
                })
                .collect()
        };
        Store { plan, queue, gen, current, workers, done, failed, errors }
    }

    pub fn compiled(&self) -> Arc<Compiled> {
        self.plan.compiled.clone()
    }

    /// 준비 집합: 곡 시각 `t`에 울리고 있어야 하는 소리 + [t, t + w) 안에 시작하는 소리가 모두 준비됐나.
    pub fn ready_at(&self, t: i64, w: i64) -> bool {
        let c = &self.plan.compiled;
        let u = slot_use(c, t);
        u.iter().enumerate().all(|(s, &(on, next))| !(on || next < t + w) || c.chart.bank.data[s].get().is_some())
    }

    /// 탐색: 남은 작업을 새 위치 기준으로 다시 줄 세운다.
    pub fn reorder(&self, t0: i64) {
        let mut q = self.queue.lock().unwrap();
        let rest = q.clone();
        *q = order(&self.plan.compiled, &rest, t0);
    }

    pub fn total(&self) -> usize {
        self.plan.jobs.len()
    }

    /// 취소 (세대를 올린다): 작업 스레드는 지금 슬롯을 게시하지 않고 멈춘다.
    pub fn cancel(&self) {
        let _ = self.current.compare_exchange(self.gen, self.gen + 1, Ordering::AcqRel, Ordering::Acquire);
    }

    pub fn join(self) {
        for w in self.workers {
            let _ = w.join();
        }
    }
}

/// 곡 시각 → 장치 프레임 (탐색 위치 계산용)
pub fn frame_of(c: &Compiled, t: i64) -> i64 {
    ns_to_frame(t, c.chart.bank.rate)
}
