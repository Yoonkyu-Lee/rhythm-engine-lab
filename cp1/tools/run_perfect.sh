#!/usr/bin/env bash
# CP1-4 4-3: PERFECT 누름 — 경로 × 고장. 쓰는 법 (cp1/에서): bash tools/run_perfect.sh
# 고장: A = 오디오 스레드 정지(150 ms, 50 ms), B = 판정 스레드 정지 500 ms, C = 입력 스레드 정지 300 ms
set -u
T=./target/release/testbench.exe
FAULTS=("" "A@10:150,A@20:50" "B@10:500" "C@10:300")
for f in "${FAULTS[@]}"; do
  $T sched --mode perfect --d-ms 43 --secs 30 ${f:+--faults $f} 2>&1 | grep -E "기록"
  CP1_EXCL_PERIOD_MS=5 $T sched --mode perfect --d-ms 11 --secs 30 --wasapi exclusive ${f:+--faults $f} 2>&1 | grep -E "기록"
  $T asiorun --mode perfect --sound click --d-ms 3 --buffer 64 --secs 30 ${f:+--faults $f} 2>&1 | grep -E "기록"
done
