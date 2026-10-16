#!/usr/bin/env bash
# CP2 2차 2-3·2-4: 고장을 넣은 실행 — 오디오 경로 × 프로필. 곡 하나에 고장 다섯 개 (15초 간격).
#   오디오 150 ms, 화면 300 ms, 판정 500 ms, 입력 300 ms, 오디오 1초
# 쓰는 법 (cp2/에서): bash tools/run_faults_v2.sh [곡.rtc] [프로필들] [경로들]
#   예: bash tools/run_faults_v2.sh content/bms_unshakable_6_11/chart.rtc "ours" "shared exclusive asio"
set -u
SONG=${1:-content/bms_unshakable_6_11/chart.rtc}
PROFILES=${2:-ours}
PATHS=${3:-shared exclusive asio}
F=audio@15:150,render@30:300,judge@45:500,input@60:300,audio@75:1000
NAME=$(basename "$(dirname "$SONG")")
for prof in $PROFILES; do
  for p in $PATHS; do
    timeout 400 ./target/release/cp2game.exe run "$SONG" --audio $p --profile $prof --bot --quit-at-end --faults $F --tag f-$NAME-$prof-$p 2>&1 | tail -1 | sed "s/^/$prof $p: /"
  done
done
