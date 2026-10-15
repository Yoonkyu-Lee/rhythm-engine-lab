#!/usr/bin/env bash
# CP2 2-6 리플레이 재현: 일부러 흔들린 입력(무작위 오차 → 여러 등급·빈 누름·MISS) + 고장을 넣은 실행을 리플레이로 다시 판정해 맞댄다.
# 쓰는 법 (cp2/에서): bash tools/run_replay.sh
set -u
F=audio@20:150,render@40:300,judge@60:500,input@80:300,audio@100:50
G=./target/release/cp2game.exe
$G run content/synth/synth.rtc --bot --no-capture --bot-jitter-ms 200 --faults $F --quit-at-end --tag rp-synth-jitter 2>&1 | tail -1
$G run content/djmax_1_4b_nm_t6/chart.rtc --bot --no-capture --bot-jitter-ms 120 --faults $F --quit-at-end --tag rp-blythe-jitter 2>&1 | tail -1
$G run content/djmax_72_4b_nm_t6/chart.rtc --bot --no-capture --faults $F --quit-at-end --tag rp-negnat 2>&1 | tail -1
$G run content/synth/synth.rtc --bot --no-capture --bot-adapt-ms 300 --stop-at 40 --faults audio@10.2:150,audio@20.2:150,audio@30.2:150 --tag rp-synth-adapt 2>&1 | tail -1
for r in $(ls -d runs/rp-*/ 2>/dev/null); do
  ./target/release/replay.exe "$r" | head -5
done
